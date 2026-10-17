#!/usr/bin/env bash
# Runs the log parser over each .log fixture in scripts/ci-failures-fixtures and
# checks the records it prints against the fixture's .records file. Fields are shown
# separated by tabs, and messages within a record by "|". A .job-timeout file beside
# a log holds the check-run annotation of a job that ran past its timeout-minutes,
# passed to the parser as job_timeout.
#
# Each .json fixture is a runs listing of the repository's merge_group runs, and
# its .records file is the merge queue batch that scripts/ci-failures-queue.jq
# selects for PR 7, whose head commit is from 2023-11-14 22:13:20 UTC. The rows
# are the run id, the workflow name and the conclusion ("-" while running).
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
parser="$dir/ci-failures-parse.awk"
actual="$(mktemp)"
trap 'rm -f "$actual"' EXIT

status=0
for log in "$dir"/ci-failures-fixtures/*.log; do
    expected="${log%.log}.records"
    # A job that ran past its timeout-minutes: ci-failures.sh passes the annotation
    # that says so, which the fixture keeps beside the log.
    job_timeout=""
    if [[ -f "${log%.log}.job-timeout" ]]; then
        job_timeout="$(cat "${log%.log}.job-timeout")"
    fi
    awk -v job_timeout="$job_timeout" -f "$parser" "$log" | tr '\037\036' '\t|' > "$actual"
    if ! diff -u "$expected" "$actual"; then
        echo "ci-failures parser: $(basename "$log") does not match $(basename "$expected")" >&2
        status=1
    fi
done

for runs in "$dir"/ci-failures-fixtures/*.json; do
    expected="${runs%.json}.records"
    jq -c -s -f "$dir/ci-failures-queue.jq" --arg prefix gh-readonly-queue/main/pr-7- \
        --argjson since 1700000000 --arg excluded "Claude Code" "$runs" \
        | jq -r '[.id, .name, (.conclusion // "-")] | @tsv' > "$actual"
    if ! diff -u "$expected" "$actual"; then
        echo "ci-failures queue: $(basename "$runs") does not match $(basename "$expected")" >&2
        status=1
    fi
done

# module_files maps a failing test's name to the repo files it touches. The
# function is read out of the script, so the check runs the code that ships.
check_module_files() {
    local fn name expected actual
    local -A repo_files=()
    fn="$(awk '/^module_files\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    eval "$fn"
    for name in src/gateway.rs src/memory/mod.rs src/lib.rs tests/it/gateway.rs \
        tests/it/memory.rs tests/it/support/mod.rs tests/ts_export.rs \
        src/util/fs.rs crates/core/src/util/fs.rs tests/it/smoke.rs tests/smoke.rs \
        tests/it/nested/mod.rs tests/it/nested/inner.rs; do
        repo_files["$name"]=1
    done
    (( ${#repo_files[@]} == 13 )) || { echo "ci-failures module_files: fixture has ${#repo_files[@]} files, expected 13" >&2; status=1; }
    while IFS='|' read -r name expected; do
        actual="$(module_files "$name")"
        if [[ "$actual" != "$expected" ]]; then
            echo "ci-failures module_files: $name gave '$actual', expected '$expected'" >&2
            status=1
        fi
    done <<'CASES'
gateway::server::tests::x|src/gateway.rs tests/it/gateway.rs
gateway::x|src/gateway.rs tests/it/gateway.rs
memory::x|src/memory/mod.rs tests/it/memory.rs
ts_export::roundtrip|tests/ts_export.rs
smoke::x|tests/it/smoke.rs tests/smoke.rs
nested::inner::tests::x|tests/it/nested/mod.rs tests/it/nested/inner.rs
nested::other::x|tests/it/nested/mod.rs
util::fs::tests::x|src/util/fs.rs crates/core/src/util/fs.rs
residuum_core::util::fs::tests::x|crates/core/src/util/fs.rs
skills::x|
lonely|
residuum_core::x|
CASES
}
check_module_files

# The Rust path regex is kept in three places (the CI workflow, the pre-push hook
# and this script). Each copy must be the one ci.yml names.
ci_regex="$(sed -n "s/^ *RUST_PATHS_RE: '\(.*\)'\$/\1/p" "$dir/../.github/workflows/ci.yml")"
hook_regex="$(sed -n "s/^RUST_PATHS_RE='\(.*\)'\$/\1/p" "$dir/../.githooks/pre-push")"
eval "$(grep '^readonly RUST_PATHS_RE=' "$dir/ci-failures.sh")"
if [[ -z "$ci_regex" || "$ci_regex" != "$RUST_PATHS_RE" ]]; then
    echo "ci-failures RUST_PATHS_RE differs from the one in .github/workflows/ci.yml" >&2
    status=1
fi
if [[ "$hook_regex" != "$RUST_PATHS_RE" ]]; then
    echo "ci-failures RUST_PATHS_RE differs from the one in .githooks/pre-push" >&2
    status=1
fi

# package_of maps a changed path to the package its covers check compares. Only a
# Rust path maps to a package, a pending-crates file line matches exactly, and a
# directory line matches by prefix.
check_package_of() {
    local fn name expected actual
    fn="$(awk '/^package_of\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    eval "$fn"
    # shellcheck disable=SC2034 # read by package_of, which is evaluated from ci-failures.sh
    pending_lines="src/bus/ residuum-bus
src/hub/team.rs residuum-background
src/hub/ residuum-hub
tests/it/x.rs residuum-test"
    while IFS='|' read -r name expected; do
        actual="$(package_of "$name")"
        if [[ "$actual" != "$expected" ]]; then
            echo "ci-failures package_of: $name gave '$actual', expected '$expected'" >&2
            status=1
        fi
    done <<'CASES'
crates/memory/src/x.rs|residuum-memory
src/bus/a.rs|residuum-bus
src/hub/team.rs|residuum-background
src/hub/team.rs.orig|residuum-hub
src/unknown.rs|residuum
tests/it/x.rs|residuum-test
tests/it/y.rs|residuum
build.rs|residuum
docs/x.md|
web/src/app.ts|
.github/workflows/web-checks.yml|
.github/workflows/release.yml|residuum
src/update.rs.orig|residuum
CASES
}
check_package_of

# module_covers reads the covers line of a tests/it file at the commit under test.
# The git stub serves the fixture files of a scratch directory.
check_module_covers() {
    local fn covers_root name expected actual
    # shellcheck disable=SC2034 # read by module_covers, which is evaluated from ci-failures.sh
    local sha="0123abc"
    fn="$(awk '/^module_covers\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    covers_root="$(mktemp -d)"
    # shellcheck disable=SC2317 # called by module_covers, through the evaluated function
    git() {
        [[ "$1" == show ]] || return 1
        cat "$covers_root/${2#*:}"
    }
    eval "$fn"
    mkdir -p "$covers_root/tests/it"
    printf '//! Tests the bus.\n//! covers: residuum-bus, residuum\nfn x() {}\n' >"$covers_root/tests/it/one.rs"
    printf '//! covers: residuum-a\n//! covers: residuum-b\n' >"$covers_root/tests/it/two.rs"
    printf '//!  covers: residuum-a\n' >"$covers_root/tests/it/spaced.rs"
    printf '//! covers: residuum-c,residuum-d\n' >"$covers_root/tests/it/comma.rs"
    printf 'fn x() {}\n' >"$covers_root/tests/it/none.rs"
    while IFS='|' read -r name expected; do
        actual="$(module_covers "$name" | tr '\n' ' ')"
        actual="${actual% }"
        if [[ "$actual" != "$expected" ]]; then
            echo "ci-failures module_covers: $name gave '$actual', expected '$expected'" >&2
            status=1
        fi
    done <<'CASES'
tests/it/one.rs|residuum-bus residuum
tests/it/two.rs|residuum-a
tests/it/spaced.rs|
tests/it/comma.rs|residuum-c,residuum-d
tests/it/none.rs|
tests/it/missing.rs|
CASES
    rm -rf "$covers_root"
    unset -f git
}
check_module_covers

# it_module_file maps an integration test to the file whose covers line is read.
check_it_module_file() {
    local fn name expected actual
    local -A repo_files=()
    fn="$(awk '/^it_module_file\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    eval "$fn"
    for name in src/integration/four.rs src/integration/five/mod.rs tests/it/six.rs; do
        repo_files["$name"]=1
    done
    while IFS='|' read -r name expected; do
        actual="$(it_module_file "$name")"
        if [[ "$actual" != "$expected" ]]; then
            echo "ci-failures it_module_file: $name gave '$actual', expected '$expected'" >&2
            status=1
        fi
    done <<'CASES'
integration::four::t|src/integration/four.rs
integration::five::tests::x|src/integration/five/mod.rs
integration::nine::x|
integration::four|
six::x|tests/it/six.rs
residuum_core::x|
CASES
}
check_it_module_file

# reported_jobs picks the jobs of a run to report. For an earlier run of a workflow
# on the commit, a job a later run ran again is left out, a job the later run
# skipped keeps its failure, and a cancelled job is listed only when it timed out.
check_reported_jobs() {
    local fn jobs_file actual expected
    # shellcheck disable=SC2034 # read by reported_jobs and remember_ran_jobs, which are evaluated from ci-failures.sh
    local -A ran_later=() job_timeout=()
    # shellcheck disable=SC2034 # read by reported_jobs and remember_ran_jobs, which are evaluated from ci-failures.sh
    local US=$'\037'
    fn="$(awk '/^reported_jobs\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    eval "$fn"
    fn="$(awk '/^remember_ran_jobs\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    eval "$fn"
    jobs_file="$(mktemp)"
    # The later run: Rerun ran again and passed, Skipped was skipped.
    printf '%s\n' \
        '{"id":11,"name":"Rerun","conclusion":"success","steps":[]}' \
        '{"id":12,"name":"Skipped","conclusion":"skipped","steps":[]}' >"$jobs_file"
    remember_ran_jobs "$jobs_file" Checks
    # The earlier run.
    printf '%s\n' \
        '{"id":1,"name":"Rerun","conclusion":"failure","steps":[{"name":"Test","conclusion":"failure"}]}' \
        '{"id":2,"name":"Skipped","conclusion":"failure","steps":[{"name":"Build","conclusion":"failure"},{"name":"Lint","conclusion":"failure"}]}' \
        '{"id":3,"name":"Passed","conclusion":"success","steps":[]}' \
        '{"id":4,"name":"Cancelled","conclusion":"cancelled","steps":[{"name":"Wait","conclusion":"cancelled"}]}' \
        '{"id":5,"name":"Out of time","conclusion":"cancelled","steps":[{"name":"Sleep","conclusion":"cancelled"}]}' >"$jobs_file"
    job_timeout[5]="The job has exceeded the maximum execution time of 1m0s"
    actual="$(reported_jobs "$jobs_file" Checks true)"
    expected=$'2\tSkipped\tfailure\tBuild + Lint\n5\tOut of time\tcancelled\tSleep'
    if [[ "$actual" != "$expected" ]]; then
        echo "ci-failures reported_jobs: an earlier run gave '$actual', expected '$expected'" >&2
        status=1
    fi
    actual="$(reported_jobs "$jobs_file" Checks false)"
    expected=$'1\tRerun\tfailure\tTest\n2\tSkipped\tfailure\tBuild + Lint\n4\tCancelled\tcancelled\tWait\n5\tOut of time\tcancelled\tSleep'
    if [[ "$actual" != "$expected" ]]; then
        echo "ci-failures reported_jobs: the latest run gave '$actual', expected '$expected'" >&2
        status=1
    fi
    # The later run was of another workflow, so it reruns none of these jobs.
    actual="$(reported_jobs "$jobs_file" Other true)"
    expected=$'1\tRerun\tfailure\tTest\n2\tSkipped\tfailure\tBuild + Lint\n5\tOut of time\tcancelled\tSleep'
    if [[ "$actual" != "$expected" ]]; then
        echo "ci-failures reported_jobs: another workflow's earlier run gave '$actual', expected '$expected'" >&2
        status=1
    fi
    rm -f "$jobs_file"
}
check_reported_jobs

# skip_earlier_run leaves out an earlier run only once it has finished with nothing to
# report. An earlier run still in progress is kept, so its failures so far are shown.
check_skip_earlier_run() {
    local fn
    fn="$(awk '/^skip_earlier_run\(\) \{/,/^\}/' "$dir/ci-failures.sh")"
    eval "$fn"
    if ! skip_earlier_run true completed ""; then
        echo "ci-failures skip_earlier_run: a finished earlier run with no rows was kept" >&2
        status=1
    fi
    if skip_earlier_run true in_progress ""; then
        echo "ci-failures skip_earlier_run: an earlier run still in progress was skipped" >&2
        status=1
    fi
    if skip_earlier_run true completed "2	Skipped	failure	Build"; then
        echo "ci-failures skip_earlier_run: a finished earlier run with rows was skipped" >&2
        status=1
    fi
    if skip_earlier_run false completed ""; then
        echo "ci-failures skip_earlier_run: the latest run was skipped" >&2
        status=1
    fi
}
check_skip_earlier_run

if (( status == 0 )); then
    echo "ci-failures parser and queue selection: every fixture matches its .records file"
fi
exit "$status"
