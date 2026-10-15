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

if (( status == 0 )); then
    echo "ci-failures parser and queue selection: every fixture matches its .records file"
fi
exit "$status"
