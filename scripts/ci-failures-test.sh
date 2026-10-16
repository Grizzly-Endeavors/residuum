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

if (( status == 0 )); then
    echo "ci-failures parser and queue selection: every fixture matches its .records file"
fi
exit "$status"
