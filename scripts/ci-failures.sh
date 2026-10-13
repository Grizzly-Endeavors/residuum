#!/usr/bin/env bash
# Lists the failing tests of the latest CI runs for a pull request, a branch, a
# release tag or a commit, grouped by job, so one pass fixes everything that
# broke. Each failure carries two flags: whether this change touches its test
# file, and whether the same test failed on another branch recently, which
# marks a known flake.
#
# Usage: scripts/ci-failures.sh [PR-number | branch | vYYYY.MM.DD[-N] | commit]
#        `just ci-failures` runs it for the current branch.
#
# touched          the test's file is in the change: the diff against the merge
#                  base with origin/main, or for a release tag the diff since the
#                  previous release tag
# seen elsewhere   the same test failed in a run on another branch or tag within
#                  the last 14 days, searching the 150 most recent runs
#
# Failed jobs' logs are cached under target/ci-failures-cache/<run>-<attempt>/,
# so a repeat call only asks GitHub about runs it has not seen.
#
# Exit status: 0 once the report is printed, failures listed or not; 1 when
# GitHub or git could not be queried.
set -euo pipefail

readonly LOOKBACK_DAYS=14
readonly LOOKBACK_RUNS=150
readonly MAX_PER_JOB=20
readonly WORKFLOWS_JSON='["Rust PR Checks","Web PR Checks","Cross-Platform Checks","Release"]'
readonly US=$'\037'
readonly RS=$'\036'

die() {
    echo "ci-failures: $*" >&2
    exit 1
}

for tool in gh jq git awk; do
    command -v "$tool" >/dev/null || die "$tool is required"
done

# Turns one failed job's log into records, one per failing test (or lint item),
# separated by US; messages within a record are separated by RS. A log with no
# recognized failure becomes one "unparsed" record holding the step's last lines.
read -r -d '' PARSE_AWK <<'AWK' || true
BEGIN {
    FS = "\n"
    US = "\037"
    RS2 = "\036"
    MAXMSG = 3
    n = 0
    nl = 0
    cur_test = ""
    collect = 0
    in_stress = 0
    pwleft = 0
    vtleft = 0
    esl_file = ""
    cl_pending = 0
}

function trim(s) {
    sub(/^[ \t\r]+/, "", s)
    sub(/[ \t\r]+$/, "", s)
    return s
}

function getrec(kind, key, title, files,    i) {
    if (key in kidx) {
        i = kidx[key]
        rcount[i]++
        isnew = 0
        return i
    }
    n++
    i = n
    kidx[key] = i
    rk[i] = kind
    rkey[i] = key
    rtitle[i] = title
    rfiles[i] = files
    rcount[i] = 1
    rproj[i] = ""
    rmsg[i] = ""
    rnmsg[i] = 0
    isnew = 1
    return i
}

# A Rust test is named once per block that mentions it (the progress line, the
# stdout header, the panic, the summary), so its count stays at one.
function rustrec(name,    i) {
    i = getrec("rust", "rust|" name, name, "")
    rcount[i] = 1
    return i
}

function addmsg(i, t) {
    if (t == "" || rnmsg[i] >= MAXMSG) return
    rmsg[i] = (rnmsg[i] > 0 ? rmsg[i] RS2 : "") t
    rnmsg[i]++
}

function addfile(i, f) {
    if (f == "") return
    if (index(" " rfiles[i] " ", " " f " ") > 0) return
    rfiles[i] = (rfiles[i] == "" ? f : rfiles[i] " " f)
}

function addproj(i, p) {
    if (index(", " rproj[i] ", ", ", " p ", ") > 0) return
    rproj[i] = (rproj[i] == "" ? p : rproj[i] ", " p)
}

{
    line = $0
    sub(/\r$/, "", line)
    sub(/^[^\t]*\t[^\t]*\t[0-9T:.-]+Z ?/, "", line)
    nl++
    L[nl] = line
}

# Rust: quiet-mode progress names, the per-test stdout blocks, and the panic.
line ~ /^---- .* stdout ----$/ {
    cur_test = line
    sub(/^---- /, "", cur_test)
    sub(/ stdout ----$/, "", cur_test)
    rustrec(cur_test)
    next
}
line ~ / --- FAILED$/ {
    t = line
    sub(/ --- FAILED$/, "", t)
    sub(/^.* /, "", t)
    if (t != "") rustrec(t)
    next
}
line ~ /^test .* \.\.\. FAILED$/ {
    t = line
    sub(/^test /, "", t)
    sub(/ \.\.\. FAILED$/, "", t)
    rustrec(t)
    next
}
line ~ /has been running for over/ {
    t = line
    sub(/^test /, "", t)
    sub(/ has been running.*$/, "", t)
    if (("rust|" t) in kidx) addmsg(kidx["rust|" t], "still running after 60 seconds (the hang guard)")
    next
}
line ~ /panicked at / {
    name = cur_test
    if (line ~ /thread '/) {
        name = line
        sub(/^.*thread '/, "", name)
        sub(/'.*$/, "", name)
    }
    f = line
    sub(/^.*panicked at /, "", f)
    sub(/:[0-9]+:[0-9]+:?$/, "", f)
    if (name != "") {
        i = rustrec(name)
        if (f ~ /\.rs$/) addfile(i, f)
        collect = i
        ncollect = 0
    }
    next
}
collect > 0 {
    if (line ~ /^(failures:|----|test result:|note:|error:)/) {
        collect = 0
    } else if (line ~ /[^ \t]/) {
        addmsg(collect, trim(line))
        ncollect++
        if (ncollect >= MAXMSG) collect = 0
    }
}

# Stress: the names that failed in one of the concurrent copies.
line ~ /^Failures \(count out of / {
    in_stress = 1
    copies = line
    sub(/^Failures \(count out of /, "", copies)
    copies = copies + 0
    next
}
in_stress && line ~ /^error:/ {
    in_stress = 0
    next
}
in_stress && line ~ /^ +[0-9]+ [^ ]+$/ {
    split(trim(line), parts, " ")
    i = rustrec(parts[2])
    addmsg(i, "failed " parts[1] " of " copies " copies under load")
    next
}

# Playwright: "  N) [project] › spec:line:col › title @tag", then Error/Expected/Received.
line ~ /^ *[0-9]+\) \[[^]]+\] › / {
    s = line
    proj = s
    sub(/^ *[0-9]+\) \[/, "", proj)
    sub(/\].*$/, "", proj)
    sub(/^ *[0-9]+\) \[[^]]+\] › /, "", s)
    file = ""
    title = s
    if (match(s, /:[0-9]+:[0-9]+ › /)) {
        file = substr(s, 1, RSTART - 1)
        title = substr(s, RSTART + RLENGTH)
    }
    sub(/( @[^ ]+)+ *$/, "", title)
    title = trim(title)
    i = getrec("playwright", "pw|" file "|" title, file " › " title, (file == "" ? "" : "web/" file))
    addproj(i, proj)
    pwrec = i
    pwleft = (isnew ? 12 : 0)
    next
}
pwleft > 0 {
    pwleft--
    if (line ~ /^ +Error: /) {
        addmsg(pwrec, trim(line))
    } else if (line ~ /^ +(Expected|Received): / && line !~ /__screenshots__|test-results\//) {
        addmsg(pwrec, trim(line))
    } else if (line ~ / are different/) {
        addmsg(pwrec, trim(line))
    }
}

# Vitest: "FAIL  file > suite > test", then the assertion message.
line ~ /^ *FAIL  / {
    s = line
    sub(/^ *FAIL +/, "", s)
    vtleft = 0
    if (index(s, " > ") > 0) {
        file = s
        sub(/ > .*$/, "", file)
        i = getrec("vitest", "vt|" s, s, "web/" file)
        if (isnew) {
            vtrec = i
            vtleft = 10
        }
    }
    next
}
vtleft > 0 {
    vtleft--
    if (rnmsg[vtrec] == 0 && line ~ /(Error|expected)/) addmsg(vtrec, trim(line))
}

# ESLint and Stylelint: an absolute file path, then "line:col  severity  message  rule".
line ~ /^\/[^ ]+\.(svelte|ts|tsx|js|mjs|cjs|css)$/ {
    esl_file = line
    sub(/^\/home\/runner\/_work\/[^\/]+\/[^\/]+\//, "", esl_file)
    next
}
esl_file != "" && line ~ /^ +[0-9]+:[0-9]+ +(error|warning|✖) / {
    loc = trim(line)
    sub(/ .*$/, "", loc)
    rest = trim(line)
    sub(/^[0-9]+:[0-9]+ +/, "", rest)
    sub(/^[^ ]+ +/, "", rest)
    rule = rest
    sub(/^.* /, "", rule)
    msg = rest
    sub(/ +[^ ]+$/, "", msg)
    i = getrec("lint", "lint|" esl_file "|" rule, esl_file " " rule, esl_file)
    if (isnew) addmsg(i, loc "  " msg)
    next
}

# Cargo and clippy: "error[code]: message", then "  --> file:line:col".
line ~ /^(error|warning)(\[[A-Za-z0-9_]+\])?: / && line !~ /could not compile|test failed|tests failed|aborting due to|didn.t exit/ {
    cmsg = line
    sub(/^(error|warning)(\[[A-Za-z0-9_]+\])?: /, "", cmsg)
    cl_pending = 1
    next
}
cl_pending == 1 {
    cl_pending = 0
    if (line ~ /^ +--> /) {
        loc = line
        sub(/^ +--> /, "", loc)
        f = loc
        sub(/:[0-9]+:[0-9]+$/, "", f)
        i = getrec("clippy", "clippy|" f "|" cmsg, f ": " cmsg, f)
        if (isnew) addmsg(i, loc)
        next
    }
}

# cargo fmt --check: "Diff in <file> at line N:".
line ~ /^Diff in / {
    f = line
    sub(/^Diff in /, "", f)
    sub(/ at line [0-9]+:?$/, "", f)
    sub(/^\/home\/runner\/_work\/[^\/]+\/[^\/]+\//, "", f)
    i = getrec("fmt", "fmt|" f, "rustfmt: " f, f)
    if (isnew) addmsg(i, "not formatted; cargo fmt --all rewrites it")
    next
}

END {
    for (i = 1; i <= n; i++) {
        print rk[i] US rkey[i] US rtitle[i] US rfiles[i] US rcount[i] US rproj[i] US rmsg[i]
    }
    if (n == 0) {
        e = nl + 1
        for (k = 1; k <= nl; k++) {
            if (L[k] ~ /^##\[error\]/) {
                e = k
                break
            }
        }
        cnt = 0
        for (k = e - 1; k >= 1 && cnt < 15; k--) {
            t = L[k]
            if (t ~ /[^ \t]/ && t !~ /^##\[/ && t !~ /\[36;1m/ && t !~ /^shell: /) {
                cnt++
                picked[cnt] = trim(t)
            }
        }
        msgs = ""
        for (k = cnt; k >= 1; k--) {
            msgs = msgs (k < cnt ? RS2 : "") picked[k]
        }
        print "unparsed" US US US US "0" US US msgs
    }
}
AWK

cache="$(git rev-parse --show-toplevel)/target/ci-failures-cache"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/jobs" "$work/rec"

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)" || die "could not resolve the GitHub repository"
export repo cache work

# --- what to look at ----------------------------------------------------------

arg="${1:-$(git rev-parse --abbrev-ref HEAD)}"
change_word="change"
base_sha=""
base_desc=""

if [[ "$arg" =~ ^[0-9]+$ ]]; then
    pr_json="$(gh pr view "$arg" --repo "$repo" --json headRefName,headRefOid 2>/dev/null)" \
        || die "no pull request #$arg in $repo"
    sha="$(jq -r .headRefOid <<<"$pr_json")"
    label="PR #$arg ($(jq -r .headRefName <<<"$pr_json"))"
    change_word="PR"
elif [[ "$arg" =~ ^v[0-9]{4}\.[0-9]{2}\.[0-9]{2}(-[0-9]+)?$ ]]; then
    git fetch --quiet --force --tags origin || die "could not fetch tags from origin"
    sha="$(git rev-parse --verify --quiet "refs/tags/$arg^{commit}")" || die "no tag $arg"
    label="tag $arg"
    previous="$(git describe --tags --abbrev=0 --match 'v*' "$sha^" 2>/dev/null || true)"
    if [[ -n "$previous" ]]; then
        base_sha="$(git rev-parse "$previous^{commit}")"
        base_desc="previous tag $previous"
    fi
elif [[ -n "$(git ls-remote --heads origin "refs/heads/$arg" 2>/dev/null)" ]]; then
    sha="$(git ls-remote --heads origin "refs/heads/$arg" | cut -f1)"
    label="branch $arg"
    local_tip="$(git rev-parse --verify --quiet "refs/heads/$arg" || true)"
    if [[ -n "$local_tip" && "$local_tip" != "$sha" ]]; then
        echo "note: local $arg is at ${local_tip:0:7}, origin has ${sha:0:7}; reporting origin's runs" >&2
    fi
elif [[ "$arg" =~ ^[0-9a-f]{40}$ ]] || git rev-parse --verify --quiet "$arg^{commit}" >/dev/null 2>&1; then
    if ! git cat-file -e "$arg^{commit}" 2>/dev/null; then
        git fetch --quiet origin "$arg" 2>/dev/null || die "could not fetch commit $arg from origin"
    fi
    sha="$(git rev-parse --verify --quiet "$arg^{commit}")"
    label="commit ${sha:0:7}"
else
    die "'$arg' is not a PR number, a pushed branch, a release tag or a commit"
fi

if ! git cat-file -e "$sha^{commit}" 2>/dev/null; then
    git fetch --quiet origin "$sha" 2>/dev/null || die "could not fetch commit $sha from origin"
fi

if [[ -z "$base_sha" ]]; then
    git fetch --quiet origin main 2>/dev/null || echo "note: could not fetch origin/main; using the local copy" >&2
    base_sha="$(git merge-base origin/main "$sha" 2>/dev/null || true)"
    if [[ -n "$base_sha" ]]; then
        base_desc="merge base with origin/main ${base_sha:0:7}"
    fi
    # A commit already on main has nothing to diff against its merge base, so
    # its change is taken as everything since the previous release tag.
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
    while IFS= read -r file; do
        changed["$file"]=1
        changed_count=$((changed_count + 1))
    done < <(git diff --name-only "$base_sha" "$sha")
    changed_known=yes
fi

declare -A repo_files=()
while IFS= read -r file; do
    repo_files["$file"]=1
done < <(git ls-tree -r --name-only "$sha")

# --- runs ---------------------------------------------------------------------

query_errors=0

gh api "repos/$repo/actions/runs?head_sha=$sha&per_page=100" > "$work/target-raw.json" \
    || die "could not list CI runs for ${sha:0:7}"
jq -c --argjson wf "$WORKFLOWS_JSON" '
    [ .workflow_runs[] | . as $r | select(any($wf[]; . == $r.name)) ]
    | group_by(.name) | map(max_by(.id)) | sort_by(.name) | .[]
    | {id, name, run_attempt, status, conclusion, head_branch, head_sha, html_url}
' "$work/target-raw.json" > "$work/target.jsonl"

if [[ ! -s "$work/target.jsonl" ]]; then
    echo "CI failures for $label at ${sha:0:7}: no CI runs for this commit yet."
    exit 0
fi

target_branch="$(jq -r '.head_branch' "$work/target.jsonl" | head -n 1)"

# Recent completed runs on other branches, for the seen-elsewhere flag.
lookback_ok=yes
cutoff="$(date -u -d "-$LOOKBACK_DAYS days" +%Y-%m-%dT%H:%M:%SZ)"
: > "$work/recent.jsonl"
for page in 1 2; do
    if gh api "repos/$repo/actions/runs?status=completed&per_page=100&page=$page" > "$work/recent-page.json" 2>/dev/null; then
        jq -c '.workflow_runs[]' "$work/recent-page.json" >> "$work/recent.jsonl"
    else
        echo "warning: could not list recent runs; the seen-elsewhere flag is unknown" >&2
        lookback_ok=no
        query_errors=$((query_errors + 1))
        break
    fi
done

: > "$work/look.jsonl"
if [[ "$lookback_ok" == yes && -s "$work/recent.jsonl" ]]; then
    jq -s -c --argjson wf "$WORKFLOWS_JSON" --arg cut "$cutoff" --arg br "$target_branch" \
        --arg sha "$sha" --argjson lim "$LOOKBACK_RUNS" '
        .[0:$lim][]
        | select(.conclusion == "failure" and .created_at >= $cut and .head_branch != $br and .head_sha != $sha)
        | select(.name as $n | any($wf[]; . == $n))
        | {id, name, run_attempt, head_branch, html_url}
    ' "$work/recent.jsonl" > "$work/look.jsonl"
fi

# --- jobs and logs --------------------------------------------------------------

fetch_jobs() { # run attempt status
    local dest="$work/jobs/$1-$2.jsonl" cached="$cache/$1-$2/jobs.jsonl"
    if [[ "$3" == completed && -s "$cached" ]]; then
        cp "$cached" "$dest"
        return 0
    fi
    if ! gh api --paginate "repos/$repo/actions/runs/$1/jobs?per_page=100" | jq -c '.jobs[]' > "$dest.part"; then
        rm -f "$dest.part"
        echo "could not list the jobs of run $1" >&2
        return 1
    fi
    mv "$dest.part" "$dest"
    if [[ "$3" == completed ]]; then
        mkdir -p "$(dirname "$cached")"
        cp "$dest" "$cached"
    fi
}

fetch_log() { # run attempt job
    local out="$cache/$1-$2/$3.log"
    if [[ -s "$out" ]]; then
        return 0
    fi
    mkdir -p "$(dirname "$out")"
    if gh run view --repo "$repo" --job "$3" --log-failed > "$out.part" 2>/dev/null; then
        mv "$out.part" "$out"
    else
        rm -f "$out.part"
        echo "could not fetch the log of job $3 in run $1" >&2
        return 1
    fi
}
export -f fetch_jobs fetch_log

{
    jq -r '"\(.id) \(.run_attempt) \(.status)"' "$work/target.jsonl"
    jq -r '"\(.id) \(.run_attempt) \(.status)"' "$work/look.jsonl"
} | xargs -r -P 8 -L 1 bash -c 'fetch_jobs "$@"' _ || query_errors=$((query_errors + 1))

# Failed jobs, as "run attempt job" lines, for the target runs and the lookback.
failed_jobs() { # pairs file of "run attempt"
    local run attempt
    while read -r run attempt; do
        jq -r --arg r "$run" --arg a "$attempt" \
            'select(.conclusion == "failure" or .conclusion == "timed_out") | "\($r) \($a) \(.id)"' \
            "$work/jobs/$run-$attempt.jsonl" 2>/dev/null || true
    done < "$1"
}

jq -r '"\(.id) \(.run_attempt)"' "$work/target.jsonl" > "$work/target-pairs.txt"
jq -r '"\(.id) \(.run_attempt)"' "$work/look.jsonl" > "$work/look-pairs.txt"
failed_jobs "$work/target-pairs.txt" > "$work/fetch-target.txt"
failed_jobs "$work/look-pairs.txt" > "$work/fetch-look.txt"

cat "$work/fetch-target.txt" "$work/fetch-look.txt" \
    | xargs -r -P 8 -L 1 bash -c 'fetch_log "$@"' _ || true

parse_job() { # run attempt job -> records in $work/rec; fails when the log is missing
    local log="$cache/$1-$2/$3.log"
    if [[ ! -f "$log" ]]; then
        return 1
    fi
    awk "$PARSE_AWK" "$log" > "$work/rec/$1-$3.tsv"
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
    if [[ -n "${repo_files[tests/${segs[0]}.rs]:-}" ]]; then
        echo "tests/${segs[0]}.rs"
    fi
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
unparsed_n=0
running_n=0
while IFS= read -r run_line; do
    IFS=$'\t' read -r run_id attempt status conclusion name url < <(
        jq -r '[.id, .run_attempt, .status, (.conclusion // "-"), .name, .html_url] | @tsv' <<<"$run_line"
    )
    if [[ "$status" == completed ]]; then
        state="$conclusion"
    else
        state="in progress"
        running_n=$((running_n + 1))
    fi
    echo
    echo "== $name - run $run_id - attempt $attempt - $state - $url"

    jobs_file="$work/jobs/$run_id-$attempt.jsonl"
    if [[ ! -f "$jobs_file" ]]; then
        echo "   could not list this run's jobs; see the run page"
        query_errors=$((query_errors + 1))
        continue
    fi
    if [[ "$status" != completed ]]; then
        finished="$(jq -s '[.[] | select(.status == "completed")] | length' "$jobs_file")"
        all_jobs="$(jq -s 'length' "$jobs_file")"
        echo "   still running: $finished of $all_jobs jobs finished; the failures so far are listed"
    fi

    while IFS=$'\t' read -r job_id job_name steps; do
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
        hidden=0
        while IFS="$US" read -r kind key title files count projects msgs; do
            total=$((total + 1))
            if [[ "$kind" == unparsed ]]; then
                unparsed_n=$((unparsed_n + 1))
                if (( shown < MAX_PER_JOB )); then
                    shown=$((shown + 1))
                    echo "     no recognized test failure; the last lines of the step:"
                    if [[ -n "$msgs" ]]; then
                        IFS="$RS" read -r -a msg_arr <<<"$msgs"
                        for m in "${msg_arr[@]}"; do
                            echo "       ${m:0:220}"
                        done
                    fi
                else
                    hidden=$((hidden + 1))
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
                for f in $files; do
                    if [[ -n "${changed[$f]:-}" ]]; then
                        touched=yes
                        break
                    fi
                done
            fi
            seen="$(seen_on "$key")"
            if [[ "$touched" == yes ]]; then
                touched_n=$((touched_n + 1))
            fi
            if [[ -n "$seen" ]]; then
                seen_n=$((seen_n + 1))
            fi

            if (( shown >= MAX_PER_JOB )); then
                hidden=$((hidden + 1))
                continue
            fi
            shown=$((shown + 1))

            heading="     - $title"
            if [[ -n "$projects" ]]; then
                heading+=" [$projects]"
            elif (( count > 1 )); then
                heading+=" (x$count)"
            fi
            echo "$heading"
            echo "         touched: $touched   seen elsewhere: ${seen:-no}"
            if [[ -n "$msgs" ]]; then
                IFS="$RS" read -r -a msg_arr <<<"$msgs"
                for m in "${msg_arr[@]}"; do
                    echo "         ${m:0:220}"
                done
            fi
        done < "$rec"

        if (( hidden > 0 )); then
            echo "     ... $hidden more in this job; see $url"
        fi
    done < <(
        jq -r 'select(.conclusion == "failure" or .conclusion == "timed_out")
            | [.id, .name, ([.steps[] | select(.conclusion == "failure") | .name] | join(" + "))] | @tsv' \
            "$jobs_file"
    )
done < "$work/target.jsonl"

echo
summary="Summary: $total failing, $touched_n touched by this $change_word"
if [[ "$changed_known" == no ]]; then
    summary="Summary: $total failing, touched unknown (no base to diff against)"
fi
if [[ "$lookback_ok" == yes ]]; then
    summary+=", $seen_n seen on other branches"
else
    summary+=", seen elsewhere unknown"
fi
summary+="."
if (( unparsed_n > 0 )); then
    summary+=" $unparsed_n step(s) had no recognized test; their last lines are shown."
fi
if (( running_n > 0 )); then
    summary+=" $running_n run(s) still in progress; the list is partial."
fi
echo "$summary"

if (( query_errors > 0 )); then
    echo "ci-failures: $query_errors query(ies) failed; the report above may be incomplete" >&2
    exit 1
fi
