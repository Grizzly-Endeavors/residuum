#!/usr/bin/env bash
# Lists the failing tests of the latest CI runs for a pull request, a branch, a
# release tag or a commit, grouped by job and step, so one pass fixes everything
# that broke. Each failure carries two flags. touched: this change edits a file the
# failure names. seen elsewhere: the same failure was in a failed run on another
# branch or tag within the last 14 days. A failure that is not touched but is seen
# elsewhere is marked likely flake. Every other failure is presumed to come from
# this change.
#
# For a PR number whose own runs show no failure, the latest merge queue batch of
# the PR is listed as well when it did not succeed: a PR ejected from the queue
# failed there, on a commit that is not its head. The batch is labelled with its
# commit.
#
# Usage: scripts/ci-failures.sh [PR-number | branch | vYYYY.MM.DD[-N] | commit]
#        `just ci-failures` runs it for the current branch.
#
# touched          a file the failure names is in the change: the test's file, and
#                  for a Rust failure also the file its panic was raised in. The
#                  diff is against the merge base with origin/main, or for a release
#                  tag the diff since the previous release tag
# seen elsewhere   the same failure was in a failed run on another branch or tag
#                  in the last 14 days. The search covers the 150 most recent
#                  completed runs, and a run that failed and was then rerun to
#                  green counts as failed.
#
# Job logs come from the jobs logs endpoint, which serves any completed job, so the
# failures of a run still in progress are listed as they finish. Logs and job lists
# of completed attempts are cached under target/ci-failures-cache/<run>-<attempt>/,
# so a repeat call only asks GitHub about runs it has not seen. The log parser is
# scripts/ci-failures-parse.awk; scripts/ci-failures-test.sh checks it against the
# fixtures in scripts/ci-failures-fixtures/.
#
# Exit status: 0 once the report is printed, failures listed or not; 1 when GitHub
# or git could not be queried, or a run failed with no failed job to list. The
# report still prints in both cases.
set -euo pipefail

readonly LOOKBACK_DAYS=14
readonly LOOKBACK_RUNS=150
readonly RUNS_PER_PAGE=100
# An agent session started by an @claude mention, not a CI check: its runs are
# never part of the report.
readonly EXCLUDED_WORKFLOW="Claude Code"
# Failures printed with their messages per job. Any beyond it are still listed,
# one line each.
readonly DETAIL_PER_JOB=20
readonly MESSAGE_WIDTH=220
readonly US=$'\037'
readonly RS=$'\036'

die() {
    echo "ci-failures: $*" >&2
    exit 1
}

# The lines of the given files, each distinct line once, in first-seen order. Two
# fetch workers must never be started on the same job.
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

# --- what to look at ----------------------------------------------------------

arg="${1:-$(git rev-parse --abbrev-ref HEAD)}"
change_word="change"
base_sha=""
base_desc=""
# The queue branch of a PR's merge batches, set for a PR number only.
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
    # A commit already on main has nothing to diff against its merge base, so its
    # change is taken as everything since the previous release tag.
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

# --- runs ---------------------------------------------------------------------

gh api --paginate "repos/$repo/actions/runs?head_sha=$sha&per_page=100" > "$work/target-raw.json" \
    || die "could not list CI runs for ${sha:0:7}"
# Every workflow that ran on the commit, latest run of each name.
jq -c -s --arg excluded "$EXCLUDED_WORKFLOW" '
    [ .[].workflow_runs[] | select(.name != $excluded) ]
    | group_by(.name) | map(max_by(.id)) | sort_by(.name) | .[]
    | {id, name, run_attempt, status, conclusion, head_branch, head_sha, html_url}
' "$work/target-raw.json" > "$work/target.jsonl" || die "could not read the CI runs for ${sha:0:7}"

if [[ ! -s "$work/target.jsonl" ]]; then
    echo "CI failures for $label at ${sha:0:7}: no CI runs for this commit yet."
    exit 0
fi

target_branch="$(jq -r '.head_branch' "$work/target.jsonl" | sed -n 1p)"

# A PR that merge queue ejected can have green runs on its head while its failing
# batch ran only on the queue branch, so its head runs are not the whole story.
# When no head run failed, the latest queue run of each workflow that was created
# after the head commit is added, unless it succeeded.
if [[ -n "$queue_prefix" ]] && ! jq -s -e 'any(.[]; .conclusion == "failure" or .conclusion == "timed_out" or .conclusion == "startup_failure")' "$work/target.jsonl" >/dev/null; then
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

# Recent completed runs on other branches, for the seen-elsewhere flag: the
# LOOKBACK_RUNS most recent, fetched in pages of at most RUNS_PER_PAGE.
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

# A run that was rerun to green still shows its failed first attempt, so the
# lookback keeps every run that failed or has more than one attempt.
: > "$work/look.jsonl"
if [[ "$lookback_ok" == yes && -s "$work/recent.jsonl" ]]; then
    # A run listed on two pages (new runs land between page fetches) is kept once.
    # The PR's own merge batches are its runs too, so they never count as elsewhere.
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

# "run attempt status" for each attempt of the target runs, and of each lookback
# run: every attempt of a rerun is listed, because each one can hold a failure.
jq -r '"\(.id) \(.run_attempt) \(.status)"' "$work/target.jsonl" > "$work/target-attempts.txt"
jq -r '. as $r | range(1; $r.run_attempt + 1) as $a | "\($r.id) \($a) completed"' \
    "$work/look.jsonl" > "$work/look-attempts.txt"

# --- jobs and logs --------------------------------------------------------------

# Copies src to dest through a temp file beside dest, so a reader never sees a
# partial copy and a failed copy leaves nothing at dest. Used for the cached job
# lists; fetch_log gives the cached logs the same treatment with a move.
publish_copy() { # src dest
    local tmp
    tmp="$(mktemp "$2.XXXXXX")" || return 1
    if cp "$1" "$tmp" && mv "$tmp" "$2"; then
        return 0
    fi
    rm -f "$tmp"
    return 1
}

# Each worker writes a temp file of its own, so two workers never race on one name.
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
export -f publish_copy fetch_jobs fetch_log

unique_lines "$work/target-attempts.txt" "$work/look-attempts.txt" \
    | xargs -r -P 8 -L 1 bash -c 'fetch_jobs "$@"' _ || true

# A failed fetch writes no file, so a missing job list is a failed query.
while read -r run attempt _; do
    if [[ ! -f "$work/jobs/$run-$attempt.jsonl" ]]; then
        query_errors=$((query_errors + 1))
    fi
done < <(unique_lines "$work/target-attempts.txt" "$work/look-attempts.txt")

# "run attempt job" lines for the failed jobs of one list of attempts.
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

failed_jobs "$work/target-attempts.txt" > "$work/fetch-target.txt"
failed_jobs "$work/look-attempts.txt" > "$work/fetch-look.txt"

unique_lines "$work/fetch-target.txt" "$work/fetch-look.txt" \
    | xargs -r -P 8 -L 1 bash -c 'fetch_log "$@"' _ || true

parse_job() { # run attempt job -> records in $work/rec; fails when the log is missing
    local log="$cache/$1-$2/$3.log"
    if [[ ! -s "$log" ]]; then
        return 1
    fi
    awk -f "$parser" "$log" > "$work/rec/$1-$3.tsv" || die "could not parse the log of job $3 in run $1"
}

# --- seen-elsewhere index ---------------------------------------------------------

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

# --- report ---------------------------------------------------------------------

module_files() { # a::b::c::test -> the repo file that holds module a::b::c
    local -a segs
    read -r -a segs <<<"${1//::/ }"
    local count=${#segs[@]} k base cand
    if (( count < 2 )); then
        return 0
    fi
    for (( k = count - 1; k >= 1; k-- )); do
        base="$(IFS=/; echo "${segs[*]:0:k}")"
        for cand in "src/$base.rs" "src/$base/mod.rs"; do
            if [[ -n "${repo_files[$cand]:-}" ]]; then
                echo "$cand"
                return 0
            fi
        done
    done
    for cand in "tests/it/${segs[0]}.rs" "tests/${segs[0]}.rs"; do
        if [[ -n "${repo_files[$cand]:-}" ]]; then
            echo "$cand"
            return 0
        fi
    done
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
while IFS= read -r run_line; do
    IFS=$'\t' read -r run_id attempt status conclusion name url branch head < <(
        jq -r '[.id, .run_attempt, .status, (.conclusion // "-"), .name, .html_url, .head_branch, .head_sha] | @tsv' <<<"$run_line"
    )
    if [[ "$status" == completed ]]; then
        state="$conclusion"
    else
        state="in progress"
        running_n=$((running_n + 1))
    fi
    if [[ "$branch" == gh-readonly-queue/* ]]; then
        name="$name, merge queue batch ${head:0:7}"
    fi
    echo
    echo "== $name - run $run_id - attempt $attempt - $state - $url"

    jobs_file="$work/jobs/$run_id-$attempt.jsonl"
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
        if [[ "$job_conclusion" == cancelled ]]; then
            if [[ "$conclusion" == failure ]]; then
                echo "   $job_name - cancelled, no results"
            fi
            continue
        fi
        listed=$((listed + 1))
        if [[ -n "$steps" ]]; then
            echo "   $job_name - step: $steps"
        else
            echo "   $job_name"
        fi

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
    done < <(
        jq -r 'select(.conclusion == "failure" or .conclusion == "timed_out" or .conclusion == "cancelled")
            | [.id, .name, .conclusion, ([.steps[]? | select(.conclusion == "failure") | .name] | join(" + "))] | @tsv' \
            "$jobs_file"
    )

    if (( listed == 0 )) && [[ "$conclusion" == failure || "$conclusion" == startup_failure ]]; then
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
