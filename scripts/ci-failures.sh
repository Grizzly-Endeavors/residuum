#!/usr/bin/env bash
set -euo pipefail

readonly LOOKBACK_DAYS=14
readonly LOOKBACK_RUNS=150
readonly RUNS_PER_PAGE=100
readonly EXCLUDED_WORKFLOW="Claude Code"
readonly DETAIL_PER_JOB=20
readonly MESSAGE_WIDTH=220
readonly US=$'\037'
readonly RS=$'\036'
RUST_PATHS_RE="$(sed -n "s/^ *RUST_PATHS_RE: '\(.*\)'\$/\1/p" "$(dirname "${BASH_SOURCE[0]}")/../.github/workflows/ci.yml")"
if [[ -z "$RUST_PATHS_RE" ]]; then
    echo "ci-failures: could not read RUST_PATHS_RE from .github/workflows/ci.yml" >&2
    exit 1
fi
readonly RUST_PATHS_RE

die() {
    echo "ci-failures: $*" >&2
    exit 1
}

unique_lines() {
    awk '!seen[$0]++' "$@"
}

for tool in gh jq git awk xargs; do
    command -v "$tool" >/dev/null || die "$tool is required"
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
parser="$script_dir/ci-failures-parse.awk"
[[ -f "$parser" ]] || die "the log parser is missing at $parser"

cache="$(git rev-parse --show-toplevel)/target/ci-failures-cache"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/jobs" "$work/rec"

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)" || die "could not resolve the GitHub repository"
export repo cache work

query_errors=0

arg="${1:-$(git rev-parse --abbrev-ref HEAD)}"
change_word="change"
base_sha=""
base_desc=""
queue_prefix=""

if [[ "$arg" =~ ^[0-9]+$ ]]; then
    pr_json="$(gh pr view "$arg" --repo "$repo" --json headRefName,headRefOid)" \
        || die "could not look up pull request #$arg in $repo"
    sha="$(jq -r .headRefOid <<<"$pr_json")"
    label="PR #$arg ($(jq -r .headRefName <<<"$pr_json"))"
    change_word="PR"
    queue_prefix="gh-readonly-queue/main/pr-$arg-"
elif [[ "$arg" =~ ^v[0-9]{4}\.[0-9]{2}\.[0-9]{2}(-[0-9]+)?$ ]]; then
    git fetch --quiet --force --tags origin || die "could not fetch tags from origin"
    sha="$(git rev-parse --verify --quiet "refs/tags/$arg^{commit}")" || die "no tag $arg"
    label="tag $arg"
    previous="$(git describe --tags --abbrev=0 --match 'v*' "$sha^" 2>/dev/null || true)"
    if [[ -n "$previous" ]]; then
        base_sha="$(git rev-parse "$previous^{commit}")"
        base_desc="previous tag $previous"
    fi
else
    remote_heads="$(git ls-remote --heads origin "refs/heads/$arg")" \
        || die "could not reach origin to look up $arg"
    if [[ -n "$remote_heads" ]]; then
        read -r sha _ <<<"$remote_heads"
        label="branch $arg"
        local_tip="$(git rev-parse --verify --quiet "refs/heads/$arg" || true)"
        if [[ -n "$local_tip" && "$local_tip" != "$sha" ]]; then
            echo "note: local $arg is at ${local_tip:0:7}, origin has ${sha:0:7}; reporting origin's runs" >&2
        fi
    elif [[ "$arg" =~ ^[0-9a-f]{40}$ ]] || git rev-parse --verify --quiet "$arg^{commit}" >/dev/null 2>&1; then
        if ! git cat-file -e "$arg^{commit}" 2>/dev/null; then
            git fetch --quiet origin "$arg" || die "could not fetch commit $arg from origin"
        fi
        sha="$(git rev-parse --verify --quiet "$arg^{commit}")"
        label="commit ${sha:0:7}"
    else
        die "'$arg' is not a PR number, a pushed branch, a release tag or a commit"
    fi
fi

if ! git cat-file -e "$sha^{commit}" 2>/dev/null; then
    git fetch --quiet origin "$sha" || die "could not fetch commit $sha from origin"
fi

if [[ -z "$base_sha" ]]; then
    git fetch --quiet origin main 2>/dev/null || echo "note: could not fetch origin/main; using the local copy" >&2
    base_sha="$(git merge-base origin/main "$sha" 2>/dev/null || true)"
    if [[ -n "$base_sha" ]]; then
        base_desc="merge base with origin/main ${base_sha:0:7}"
    fi
    if [[ "$base_sha" == "$sha" ]]; then
        previous="$(git describe --tags --abbrev=0 --match 'v*' "$sha^" 2>/dev/null || true)"
        base_sha=""
        if [[ -n "$previous" ]]; then
            base_sha="$(git rev-parse "$previous^{commit}")"
            base_desc="previous tag $previous"
        fi
    fi
fi

declare -A changed=()
changed_known=no
changed_count=0
if [[ -n "$base_sha" ]]; then
    changed_list="$(git diff --name-only "$base_sha" "$sha")" || die "could not diff ${sha:0:7} against ${base_sha:0:7}"
    while IFS= read -r file; do
        [[ -n "$file" ]] || continue
        changed["$file"]=1
        changed_count=$((changed_count + 1))
    done <<<"$changed_list"
    changed_known=yes
fi

declare -A repo_files=()
repo_list="$(git ls-tree -r --name-only "$sha")" || die "could not list the files at ${sha:0:7}"
while IFS= read -r file; do
    repo_files["$file"]=1
done <<<"$repo_list"

gh api --paginate "repos/$repo/actions/runs?head_sha=$sha&per_page=100" > "$work/target-raw.json" \
    || die "could not list CI runs for ${sha:0:7}"
jq -c -s --arg excluded "$EXCLUDED_WORKFLOW" '
    [ .[].workflow_runs[] | select(.name != $excluded) ]
    | group_by(.name) | .[] | sort_by(-.id) | to_entries[]
    | .value + {earlier: (.key > 0)}
    | {id, name, run_attempt, status, conclusion, head_branch, head_sha, html_url, earlier}
' "$work/target-raw.json" > "$work/target.jsonl" || die "could not read the CI runs for ${sha:0:7}"

if [[ -n "$queue_prefix" ]]; then
    target_branch="$(jq -r .headRefName <<<"$pr_json")"
else
    target_branch="$(jq -r '.head_branch' "$work/target.jsonl" | sed -n 1p)"
fi

if [[ -n "$queue_prefix" ]]; then
    head_epoch="$(git show -s --format=%ct "$sha")" || die "could not read the commit time of ${sha:0:7}"
    if gh api "repos/$repo/actions/runs?event=merge_group&per_page=100" > "$work/queue-raw.json"; then
        jq -c -s -f "$script_dir/ci-failures-queue.jq" --arg prefix "$queue_prefix" \
            --argjson since "$head_epoch" --arg excluded "$EXCLUDED_WORKFLOW" "$work/queue-raw.json" \
            >> "$work/target.jsonl" || die "could not read the merge queue runs"
    else
        echo "warning: could not list the merge queue runs; a batch that failed is not listed" >&2
        query_errors=$((query_errors + 1))
    fi
fi

if [[ ! -s "$work/target.jsonl" ]]; then
    echo "CI failures for $label at ${sha:0:7}: no CI runs for this commit yet."
    exit 0
fi

lookback_ok=yes
: > "$work/recent.jsonl"
left=$LOOKBACK_RUNS
for (( page = 1; left > 0; page++ )); do
    per_page=$(( left < RUNS_PER_PAGE ? left : RUNS_PER_PAGE ))
    left=$(( left - per_page ))
    if gh api "repos/$repo/actions/runs?status=completed&per_page=$per_page&page=$page" > "$work/recent-page.json"; then
        jq -c '.workflow_runs[]' "$work/recent-page.json" >> "$work/recent.jsonl" \
            || die "could not read the recent CI runs"
    else
        echo "warning: could not list recent runs; the seen-elsewhere flag is unknown" >&2
        lookback_ok=no
        query_errors=$((query_errors + 1))
        break
    fi
done

: > "$work/look.jsonl"
if [[ "$lookback_ok" == yes && -s "$work/recent.jsonl" ]]; then
    jq -s -c --arg excluded "$EXCLUDED_WORKFLOW" --arg br "$target_branch" --arg sha "$sha" \
        --arg own "$queue_prefix" --argjson days "$LOOKBACK_DAYS" '
        (now - $days * 86400 | todate) as $cut
        | [ .[]
            | select(.name != $excluded and .head_branch != $br and .head_sha != $sha and .created_at >= $cut)
            | select($own == "" or ((.head_branch // "") | startswith($own) | not))
            | select(.conclusion == "failure" or .run_attempt > 1)
            | {id, name, run_attempt, head_branch, html_url} ]
        | unique_by(.id)
        | .[]
    ' "$work/recent.jsonl" > "$work/look.jsonl" || die "could not read the recent CI runs"
fi

jq -r '"\(.id) \(.run_attempt) \(.status)"' "$work/target.jsonl" > "$work/target-attempts.txt"
jq -r '. as $r | range(1; $r.run_attempt + 1) as $a | "\($r.id) \($a) completed"' \
    "$work/look.jsonl" > "$work/look-attempts.txt"

publish_copy() { # src dest
    local tmp
    tmp="$(mktemp "$2.XXXXXX")" || return 1
    if cp "$1" "$tmp" && mv "$tmp" "$2"; then
        return 0
    fi
    rm -f "$tmp"
    return 1
}

fetch_jobs() { # run attempt status
    set -o pipefail
    local dest="$work/jobs/$1-$2.jsonl" cached="$cache/$1-$2/jobs.jsonl" tmp
    if [[ "$3" == completed && -s "$cached" ]]; then
        cp "$cached" "$dest"
        return 0
    fi
    tmp="$(mktemp "$dest.XXXXXX")" || return 1
    if ! gh api --paginate "repos/$repo/actions/runs/$1/attempts/$2/jobs?per_page=100" | jq -c '.jobs[]' > "$tmp"; then
        rm -f "$tmp"
        echo "could not list the jobs of run $1, attempt $2" >&2
        return 1
    fi
    mv "$tmp" "$dest"
    if [[ "$3" == completed ]]; then
        mkdir -p "$(dirname "$cached")"
        publish_copy "$dest" "$cached" || echo "could not cache the jobs of run $1, attempt $2" >&2
    fi
}

fetch_log() { # run attempt job
    set -o pipefail
    local out="$cache/$1-$2/$3.log" tmp
    if [[ -s "$out" ]]; then
        return 0
    fi
    mkdir -p "$(dirname "$out")"
    tmp="$(mktemp "$out.XXXXXX")" || return 1
    if gh api --allow-escape-sequences "repos/$repo/actions/jobs/$3/logs" > "$tmp"; then
        mv "$tmp" "$out"
    else
        rm -f "$tmp"
        echo "could not fetch the log of job $3 in run $1, attempt $2" >&2
        return 1
    fi
}
fetch_annotations() { # run attempt job
    set -o pipefail
    local out="$cache/$1-$2/$3.annotations.json" tmp
    if [[ -s "$out" ]]; then
        return 0
    fi
    mkdir -p "$(dirname "$out")"
    tmp="$(mktemp "$out.XXXXXX")" || return 1
    if gh api "repos/$repo/check-runs/$3/annotations?per_page=100" > "$tmp"; then
        mv "$tmp" "$out"
    else
        rm -f "$tmp"
        echo "could not fetch the annotations of job $3 in run $1, attempt $2" >&2
        return 1
    fi
}
export -f publish_copy fetch_jobs fetch_log fetch_annotations

unique_lines "$work/target-attempts.txt" "$work/look-attempts.txt" \
    | xargs -r -P 8 -L 1 bash -c 'fetch_jobs "$@"' _ || true

while read -r run attempt _; do
    if [[ ! -f "$work/jobs/$run-$attempt.jsonl" ]]; then
        query_errors=$((query_errors + 1))
    fi
done < <(unique_lines "$work/target-attempts.txt" "$work/look-attempts.txt")

failed_jobs() { # attempts file
    local run attempt file
    while read -r run attempt _; do
        file="$work/jobs/$run-$attempt.jsonl"
        if [[ -f "$file" ]]; then
            jq -r --arg r "$run" --arg a "$attempt" \
                'select(.conclusion == "failure" or .conclusion == "timed_out") | "\($r) \($a) \(.id)"' "$file"
        fi
    done < "$1"
}

cancelled_jobs() { # attempts file
    local run attempt file
    while read -r run attempt _; do
        file="$work/jobs/$run-$attempt.jsonl"
        if [[ -f "$file" ]]; then
            jq -r --arg r "$run" --arg a "$attempt" \
                'select(.conclusion == "cancelled" and any(.steps[]?; .conclusion == "cancelled")) | "\($r) \($a) \(.id)"' "$file"
        fi
    done < "$1"
}

failed_jobs "$work/target-attempts.txt" > "$work/fetch-target.txt"
failed_jobs "$work/look-attempts.txt" > "$work/fetch-look.txt"
cancelled_jobs "$work/target-attempts.txt" > "$work/cancelled-target.txt"
cancelled_jobs "$work/look-attempts.txt" > "$work/cancelled-look.txt"

unique_lines "$work/cancelled-target.txt" "$work/cancelled-look.txt" \
    | xargs -r -P 8 -L 1 bash -c 'fetch_annotations "$@"' _ || true

declare -A job_timeout=()
for side in target look; do
    while read -r run attempt job; do
        annotations="$cache/$run-$attempt/$job.annotations.json"
        if [[ ! -s "$annotations" ]]; then
            echo "warning: could not fetch the annotations of cancelled job $job in run $run; a timeout of that job is not listed or matched elsewhere" >&2
            query_errors=$((query_errors + 1))
            continue
        fi
        timeout_msg="$(jq -r 'first(.[] | .message | select(test("exceeded the maximum execution time"))) // empty' "$annotations")" \
            || die "could not read the annotations of job $job in run $run"
        if [[ -n "$timeout_msg" ]]; then
            job_timeout["$job"]="$timeout_msg"
            echo "$run $attempt $job" >> "$work/fetch-$side.txt"
        fi
    done < "$work/cancelled-$side.txt"
done

unique_lines "$work/fetch-target.txt" "$work/fetch-look.txt" \
    | xargs -r -P 8 -L 1 bash -c 'fetch_log "$@"' _ || true

parse_job() { # run attempt job -> records in $work/rec; fails when the log is missing
    local log="$cache/$1-$2/$3.log"
    if [[ ! -s "$log" ]]; then
        return 1
    fi
    awk -v job_timeout="${job_timeout[$3]:-}" -f "$parser" "$log" > "$work/rec/$1-$3.tsv" \
        || die "could not parse the log of job $3 in run $1"
}

declare -A look_branch=()
while read -r id head; do
    look_branch["$id"]="$head"
done < <(jq -r '"\(.id) \(.head_branch)"' "$work/look.jsonl")

: > "$work/index.tsv"
while read -r run attempt job; do
    if ! parse_job "$run" "$attempt" "$job"; then
        echo "warning: could not fetch the log of job $job in run $run; the seen-elsewhere flag may be incomplete" >&2
        query_errors=$((query_errors + 1))
        continue
    fi
    awk -F "$US" -v r="$run" -v b="${look_branch[$run]:-}" '$2 != "" { print $2 "\t" r "\t" b }' \
        "$work/rec/$run-$job.tsv" >> "$work/index.tsv"
done < "$work/fetch-look.txt"

seen_on() { # key -> comma list of other branches with the same failure
    K="$1" awk -F '\t' '$1 == ENVIRON["K"] { print $3 }' "$work/index.tsv" | sort -u | paste -sd, -
}

module_files() { # a::b::c::test -> the repo files that hold module a::b::c
    local name="$1" member=no root base cand module_match k count
    local -a roots=() segs
    local out=""
    if [[ "$name" =~ ^residuum_([a-z0-9_]+)::(.+)$ ]]; then
        member=yes
        roots=("crates/${BASH_REMATCH[1]//_/-}/src")
        name="${BASH_REMATCH[2]}"
    else
        roots=("src")
        while IFS= read -r root; do
            roots+=("$root")
        done < <(
            for cand in "${!repo_files[@]}"; do
                if [[ "$cand" == crates/*/src/* ]]; then
                    echo "${cand%/src/*}/src"
                fi
            done | sort -u
        )
    fi
    read -r -a segs <<<"${name//::/ }"
    count=${#segs[@]}
    if (( count < 2 )); then
        return 0
    fi
    for root in "${roots[@]}"; do
        module_match=""
        for (( k = count - 1; k >= 1; k-- )); do
            [[ -n "$module_match" ]] && break
            base="$(IFS=/; echo "${segs[*]:0:k}")"
            for cand in "$root/$base.rs" "$root/$base/mod.rs"; do
                if [[ -n "${repo_files[$cand]:-}" ]]; then
                    module_match="$cand"
                    break
                fi
            done
        done
        if [[ -n "$module_match" ]]; then
            out="$out $module_match"
        fi
    done
    if [[ "$member" == no ]]; then
        if [[ -n "${repo_files[tests/it/${segs[0]}.rs]:-}" ]]; then
            out="$out tests/it/${segs[0]}.rs"
        fi
        if [[ -n "${repo_files[tests/it/${segs[0]}/mod.rs]:-}" ]]; then
            out="$out tests/it/${segs[0]}/mod.rs"
        fi
        if [[ -n "${repo_files[tests/it/${segs[0]}/${segs[1]}.rs]:-}" ]]; then
            out="$out tests/it/${segs[0]}/${segs[1]}.rs"
        fi
        if [[ -n "${repo_files[tests/${segs[0]}.rs]:-}" ]]; then
            out="$out tests/${segs[0]}.rs"
        fi
    fi
    echo "${out# }"
}

it_module_file() { # a::b::test -> tests/it/a.rs or tests/it/a/mod.rs
    local name="$1" seg0 cand
    if [[ "$name" != *::* || "$name" =~ ^residuum_ ]]; then
        return 0
    fi
    seg0="${name%%::*}"
    for cand in "tests/it/$seg0.rs" "tests/it/$seg0/mod.rs"; do
        if [[ -n "${repo_files[$cand]:-}" ]]; then
            echo "$cand"
            return 0
        fi
    done
}

# The packages a tests/it module's covers line names, one per line, read at the
# commit under test from its canonical line `//! covers: <package>, …`. A module
# without one names none.
module_covers() { # tests/it file -> package names
    local file="$1" content list
    content="$(git show "$sha:$file" 2>/dev/null)" || content=""
    list="$(awk '/^\/\/! covers: / { sub(/^\/\/! covers: /, ""); print; exit }' <<<"$content")"
    [[ -n "$list" ]] || return 0
    awk -v list="$list" 'BEGIN { n = split(list, names, ", "); for (i = 1; i <= n; i++) print names[i] }'
}

package_of() { # path -> package
    local path="$1"
    if ! grep -qE "$RUST_PATHS_RE" <<<"$path"; then
        return 0
    fi
    if [[ "$path" =~ ^crates/([^/]+)/ ]]; then
        echo "residuum-${BASH_REMATCH[1]}"
        return 0
    fi
    case "$path" in
    web/app-shell-csp.txt | .github/workflows/release.yml) echo residuum-hub ;;
    *) echo residuum ;;
    esac
}

print_messages() { # messages indent
    local m
    local -a msg_arr=()
    IFS="$RS" read -r -a msg_arr <<<"$1"
    for m in "${msg_arr[@]}"; do
        if (( ${#m} > MESSAGE_WIDTH )); then
            echo "$2${m:0:MESSAGE_WIDTH}…"
        else
            echo "$2$m"
        fi
    done
}

reported_jobs() { # jobs-file workflow earlier
    local id job_name rest
    while IFS=$'\t' read -r id job_name rest; do
        [[ -n "$id" ]] || continue
        if [[ "$3" == true ]]; then
            if [[ -n "${ran_later[$2$US$job_name]:-}" ]]; then
                continue
            fi
            if [[ "$rest" == cancelled* && -z "${job_timeout[$id]:-}" ]]; then
                continue
            fi
        fi
        printf '%s\t%s\t%s\n' "$id" "$job_name" "$rest"
    done < <(
        jq -r 'select(.conclusion == "failure" or .conclusion == "timed_out" or .conclusion == "cancelled")
            | (if .conclusion == "cancelled" then "cancelled" else "failure" end) as $step_end
            | [.id, .name, .conclusion, ([.steps[]? | select(.conclusion == $step_end) | .name] | join(" + "))] | @tsv' \
            "$1"
    )
}

skip_earlier_run() { # earlier status job-rows
    [[ "$1" == true && "$2" == completed && -z "$3" ]]
}

remember_ran_jobs() { # jobs-file workflow
    local job_name
    while IFS= read -r job_name; do
        ran_later["$2$US$job_name"]=1
    done < <(jq -r 'select(.conclusion != "skipped") | .name' "$1")
}

declare -A changed_pkgs=()
for file in "${!changed[@]}"; do
    changed_pkg="$(package_of "$file")"
    if [[ -n "$changed_pkg" ]]; then
        changed_pkgs["$changed_pkg"]=1
    fi
done

if [[ "$changed_known" == yes ]]; then
    diff_text="$changed_count files changed against ${base_desc}"
else
    diff_text="no base to diff against"
fi
echo "CI failures for $label at ${sha:0:7} ($diff_text)"

total=0
touched_n=0
seen_n=0
flake_n=0
unparsed_n=0
running_n=0
declare -A ran_later=()
while IFS= read -r run_line; do
    IFS=$'\t' read -r run_id attempt status conclusion name url branch head earlier < <(
        jq -r '[.id, .run_attempt, .status, (.conclusion // "-"), .name, .html_url, .head_branch, .head_sha, (.earlier // false)] | @tsv' <<<"$run_line"
    )
    jobs_file="$work/jobs/$run_id-$attempt.jsonl"
    job_rows=""
    if [[ -f "$jobs_file" ]]; then
        job_rows="$(reported_jobs "$jobs_file" "$name" "$earlier")"
        remember_ran_jobs "$jobs_file" "$name"
        if skip_earlier_run "$earlier" "$status" "$job_rows"; then
            continue
        fi
    fi
    if [[ "$status" == completed ]]; then
        state="$conclusion"
    else
        state="in progress"
        running_n=$((running_n + 1))
    fi
    if [[ "$branch" == gh-readonly-queue/* ]]; then
        name="$name, merge queue batch ${head:0:7}"
    fi
    if [[ "$earlier" == true ]]; then
        state="$state, earlier run; a later run of this workflow did not rerun the jobs below"
    fi
    echo
    echo "== $name - run $run_id - attempt $attempt - $state - $url"

    if [[ ! -f "$jobs_file" ]]; then
        echo "   could not list this run's jobs; see the run page"
        continue
    fi
    if [[ "$status" != completed ]]; then
        finished="$(jq -s '[.[] | select(.status == "completed")] | length' "$jobs_file")"
        all_jobs="$(jq -s 'length' "$jobs_file")"
        echo "   still running: $finished of $all_jobs jobs finished; the failures so far are listed"
    fi

    listed=0
    while IFS=$'\t' read -r job_id job_name job_conclusion steps; do
        [[ -n "$job_id" ]] || continue
        if [[ "$job_conclusion" == cancelled && -z "${job_timeout[$job_id]:-}" ]]; then
            if [[ "$conclusion" == failure ]]; then
                echo "   $job_name - cancelled, no results"
            fi
            continue
        fi
        listed=$((listed + 1))
        job_line="   $job_name"
        if [[ "$job_conclusion" != failure ]]; then
            job_line+=" - timed out"
        fi
        if [[ -n "$steps" ]]; then
            job_line+=" - step: $steps"
        fi
        echo "$job_line"

        if ! parse_job "$run_id" "$attempt" "$job_id"; then
            echo "     could not fetch this job's log; see the run page"
            query_errors=$((query_errors + 1))
            continue
        fi
        rec="$work/rec/$run_id-$job_id.tsv"

        shown=0
        while IFS="$US" read -r kind key title files count projects msgs; do
            total=$((total + 1))
            if [[ "$kind" == unparsed ]]; then
                unparsed_n=$((unparsed_n + 1))
                if [[ -n "$title" ]]; then
                    where="step \"$title\""
                else
                    where="the job's log"
                fi
                if (( shown < DETAIL_PER_JOB )); then
                    shown=$((shown + 1))
                    echo "     no recognized failure in $where; its last lines:"
                    print_messages "$msgs" "       "
                else
                    echo "     - no recognized failure in $where; see $url"
                fi
                continue
            fi

            if [[ "$kind" == rust ]]; then
                files="$files $(module_files "$title")"
            fi
            touched=no
            if [[ "$changed_known" == no ]]; then
                touched=unknown
            else
                read -r -a file_arr <<<"$files"
                for f in "${file_arr[@]}"; do
                    if [[ -n "${changed[$f]:-}" ]]; then
                        touched=yes
                        break
                    fi
                done
                if [[ "$touched" == no && "$kind" == rust ]]; then
                    it_file="$(it_module_file "$title")"
                    if [[ -n "$it_file" ]]; then
                        while read -r covered; do
                            if [[ -n "${changed_pkgs[$covered]:-}" ]]; then
                                touched=yes
                                break
                            fi
                        done < <(module_covers "$it_file")
                    fi
                fi
            fi
            seen="$(seen_on "$key")"
            flake=no
            if [[ "$touched" == yes ]]; then
                touched_n=$((touched_n + 1))
            fi
            if [[ -n "$seen" ]]; then
                seen_n=$((seen_n + 1))
            fi
            if [[ "$touched" == no && -n "$seen" ]]; then
                flake=yes
                flake_n=$((flake_n + 1))
            fi

            label_text="$title"
            if [[ -n "$projects" ]]; then
                label_text+=" [$projects]"
            elif (( count > 1 )); then
                label_text+=" (x$count)"
            fi
            flags="touched: $touched   seen elsewhere: ${seen:-no}"
            if [[ "$flake" == yes ]]; then
                flags+="   likely flake"
            fi
            if (( shown < DETAIL_PER_JOB )); then
                shown=$((shown + 1))
                echo "     - $label_text"
                echo "         $flags"
                print_messages "$msgs" "         "
            else
                echo "     - $label_text   $flags"
            fi
        done < "$rec"
    done <<<"$job_rows"

    if (( listed == 0 )) && [[ "$earlier" != true ]] && [[ "$conclusion" == failure || "$conclusion" == startup_failure ]]; then
        echo "   the run failed but no failed job was listed; see $url"
        total=$((total + 1))
        query_errors=$((query_errors + 1))
    fi
done < "$work/target.jsonl"

echo
summary_head="Summary"
if (( query_errors > 0 )); then
    summary_head="Summary (INCOMPLETE: $query_errors query(ies) failed)"
fi
summary="$summary_head: $total failing"
if [[ "$changed_known" == yes ]]; then
    summary+=", $touched_n touched by this $change_word"
else
    summary+=", touched unknown (no base to diff against)"
fi
if [[ "$lookback_ok" == yes ]]; then
    summary+=", $seen_n seen on other branches"
else
    summary+=", seen elsewhere unknown"
fi
if [[ "$changed_known" == yes && "$lookback_ok" == yes ]]; then
    summary+=", $flake_n likely flake(s)"
else
    summary+=", likely flakes unknown"
fi
summary+="."
if (( unparsed_n > 0 )); then
    summary+=" $unparsed_n step(s) had no recognized failure; their last lines are shown."
fi
if (( running_n > 0 )); then
    summary+=" $running_n run(s) still in progress; the list is partial."
fi
echo "$summary"

if (( query_errors > 0 )); then
    echo "ci-failures: $query_errors query(ies) failed; the report above may be incomplete" >&2
    exit 1
fi
