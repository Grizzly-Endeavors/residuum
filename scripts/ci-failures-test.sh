#!/usr/bin/env bash
# Runs the log parser over each .log fixture in scripts/ci-failures-fixtures and
# checks the records it prints against the fixture's .records file. Fields are shown
# separated by tabs, and messages within a record by "|".
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
    awk -f "$parser" "$log" | tr '\037\036' '\t|' > "$actual"
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

if (( status == 0 )); then
    echo "ci-failures parser and queue selection: every fixture matches its .records file"
fi
exit "$status"
