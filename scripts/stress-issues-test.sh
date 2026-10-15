#!/usr/bin/env bash
# Runs scripts/stress-issues.sh as a dry run over logs in scripts/ci-failures-fixtures
# and checks which issues it would open: one per failing test, a test named in two
# logs once, a crashed test binary under a title without its build hash, and one
# suite-wide issue when no log names a test.
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures="$dir/ci-failures-fixtures"
export STRESS_ISSUES_DRY_RUN=1 GITHUB_REPOSITORY=example/residuum GITHUB_RUN_ID=1 GITHUB_SHA=0000000

status=0
expect_titles() {
    local name="$1" expected="$2" actual
    shift 2
    actual="$("$dir/stress-issues.sh" "$@" | sed -n 's/^=== would open: //p')"
    if [ "$actual" != "$expected" ]; then
        echo "stress-issues: $name opens different issues than expected" >&2
        diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") >&2 || true
        status=1
    fi
}

expect_titles "a Rust stress log and a Tests log" "Nightly stress: hub::runtime::tests::changing_the_gateway_port_rebinds_the_server_and_keeps_the_agents_running
Nightly stress: gateway::startup::memory::tests::deleted_episode_prunes_its_vector_rows_on_resync
Nightly stress: hub::host::tests::reload_keeps_running_agents
Nightly stress: residuum test binary
Nightly stress: hub::host::tests::main_conversation::a_web_message_sent" \
    Rust failure "$fixtures/rust-stress-split.log" "$fixtures/two-failed-steps.log"

expect_titles "a test named in two logs" "Nightly stress: hub::runtime::tests::changing_the_gateway_port_rebinds_the_server_and_keeps_the_agents_running" \
    Rust failure "$fixtures/stress.log" "$fixtures/stress.log"

expect_titles "a web stress log" "Nightly stress: e2e/smoke/tab-title.spec.ts › the tab is titled with the agent or place the page is on" \
    Web failure "$fixtures/web-stress.log"

expect_titles "a log that names no test" "Nightly stress: the Rust run failed without naming a failing test" \
    Rust failure "$fixtures/warning-and-crash.log"

expect_titles "no log at all" "Nightly stress: the Web run failed without naming a failing test" \
    Web failure "$fixtures/does-not-exist.log"

expect_titles "a job that did not fail" "" \
    Web success "$fixtures/web-stress.log"

if (( status == 0 )); then
    echo "stress-issues: every case opens the expected issues"
fi
exit "$status"
