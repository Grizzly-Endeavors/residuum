#!/usr/bin/env bash
# Runs the log parser over each fixture in scripts/ci-failures-fixtures and checks
# the records it prints against the fixture's .records file. Fields are shown
# separated by tabs, and messages within a record by "|".
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
        status=1
    fi
done

if (( status == 0 )); then
    echo "ci-failures parser: every fixture matches its .records file"
fi
exit "$status"
