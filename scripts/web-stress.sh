#!/usr/bin/env bash
# Runs the end-to-end specs that a branch adds or changes, repeatedly and with the
# browser's CPU throttled: the web counterpart of the Rust Stress step. A race in a
# new spec then fails on the pull request that adds it, instead of flaking on some
# later, unrelated pull request. CI's Web Stress job runs it on pull requests
# (.github/workflows/quality-checks.yml), and `just web-stress` runs it here.
#
# Selected: the spec files under web/e2e that differ from the merge base with the
# base branch, including uncommitted and new files. Deleted specs drop out. Harness,
# mock and config changes (web/e2e/support, the mock, the Playwright config) select
# nothing here: the regular end-to-end run covers them.
#
# Projects: the ones `npm run e2e:fast` runs. The visual projects are left out, and
# the visual comparisons cover them.
#
# Budget: each selected spec runs WEB_STRESS_REPEAT times, lowered when the test
# instances times the repeats would exceed 400 instance-runs, so one wide change
# cannot make the job run for hours. The job's timeout-minutes is the backstop.
#
# Load: every page's CPU runs WEB_STRESS_CPU_THROTTLE times slower (Chromium's
# Emulation.setCPUThrottlingRate, applied by web/e2e/support/fixtures.ts while
# E2E_CPU_THROTTLE is set). The browser runs in the Playwright container on CI, so
# the runner's own load never reaches it; the throttle is what slows it.
#
# Arguments go to `playwright test` after the selected specs, so a --grep narrows
# the run: `just web-stress --grep "tab is titled"`.
#
# Environment:
#   WEB_STRESS_BASE           branch whose merge base the changes are taken against (default origin/main)
#   WEB_STRESS_REPEAT         most times each selected test runs (default 5)
#   WEB_STRESS_WORKERS        workers, as E2E_WORKERS (default 2). Playwright's --workers
#                             flag is not used: each worker starts its own mock servers, and
#                             the config reads E2E_WORKERS to start them.
#   WEB_STRESS_CPU_THROTTLE   CPU slow-down factor for every page; 1 is no slow-down (default 4)
#
# A failure is listed in Playwright's list output, which scripts/ci-failures.sh
# reads. A spec that fails only here is waiting on time instead of the event it
# checks: fix the wait (web/CONTRIBUTING.md, "Writing a spec"), never rerun past it.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
base_ref="${WEB_STRESS_BASE:-origin/main}"
repeat_cap="${WEB_STRESS_REPEAT:-5}"
workers="${WEB_STRESS_WORKERS:-2}"
throttle="${WEB_STRESS_CPU_THROTTLE:-4}"
instance_budget=400

require_whole() {
    local name="$1" value="$2"
    if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
        echo "error: $name must be a whole number of at least 1, got \"$value\"." >&2
        exit 1
    fi
}
require_whole WEB_STRESS_REPEAT "$repeat_cap"
require_whole WEB_STRESS_WORKERS "$workers"
require_whole WEB_STRESS_CPU_THROTTLE "$throttle"

cd "$repo"
base="$(git merge-base "$base_ref" HEAD)" || {
    echo "error: no merge base with $base_ref. Fetch it (git fetch origin main), or set WEB_STRESS_BASE." >&2
    exit 1
}

# Git prints paths from the repository root, Playwright takes them from web/.
edited="$(git diff --name-only --diff-filter=d "$base" -- 'web/e2e/**/*.spec.ts')"
added="$(git ls-files --others --exclude-standard -- 'web/e2e/**/*.spec.ts')"
changed="$(printf '%s\n%s\n' "$edited" "$added" | sed -e '/^$/d' -e 's|^web/||' | sort -u)"

# The workflow's job summary row reads this step output, so it can say what ran.
record() {
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
        echo "summary=$1" >>"$GITHUB_OUTPUT"
    fi
}

specs=()
if [ -n "$changed" ]; then
    mapfile -t specs <<<"$changed"
fi
if [ "${#specs[@]}" -eq 0 ]; then
    echo "No changed specs against $base_ref: nothing to repeat."
    record "no changed specs against $base_ref"
    exit 0
fi
echo "Changed specs against $base_ref (merge base $base):"
printf '  %s\n' "${specs[@]}"

cd "$repo/web"

# --list runs no test and starts no server. Playwright exits non-zero when no test
# matches, which a spec of visual tests alone does here, so the Total line decides
# whether anything runs and the exit status only matters when tests were listed.
list_status=0
listing="$(npm run --silent e2e:fast -- --list "${specs[@]}" "$@")" || list_status=$?
instances="$(sed -n 's/^Total: \([0-9]*\) tests\{0,1\} in .*$/\1/p' <<<"$listing")"
if ! [[ "$instances" =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$listing" >&2
    echo "error: could not read the test count from Playwright's listing: it has no single \"Total:\" line." >&2
    exit 1
fi
if [ "$instances" -eq 0 ]; then
    echo "The changed specs have no tests in these projects: nothing to repeat."
    record "changed specs have no tests in these projects"
    exit 0
fi
if [ "$list_status" -ne 0 ]; then
    printf '%s\n' "$listing" >&2
    echo "error: Playwright's listing failed with status $list_status." >&2
    exit "$list_status"
fi

repeat="$repeat_cap"
lowered=""
if (( instances * repeat > instance_budget )); then
    repeat=$(( instance_budget / instances ))
    (( repeat >= 1 )) || repeat=1
    lowered="; WEB_STRESS_REPEAT=$repeat_cap lowered by the budget of $instance_budget instance-runs"
fi

record "$(IFS=', '; echo "${specs[*]}") ($instances test instances, $repeat run(s) each, CPU throttled ${throttle}x, $workers workers)"

echo "Stress: $instances test instances, $repeat run(s) each$lowered. $workers workers, CPU throttled ${throttle}x."

status=0
E2E_WORKERS="$workers" E2E_CPU_THROTTLE="$throttle" \
    npm run --silent e2e:fast -- --repeat-each="$repeat" --retries=0 "${specs[@]}" "$@" || status=$?

if [ "$status" -ne 0 ]; then
    echo "error: the stress run failed ($repeat run(s) each, $workers workers, CPU throttled ${throttle}x): a spec failed under load, or the run did not start. The failures and the report (web/playwright-report) are above. A spec that fails only under load is waiting on time instead of the event it checks: fix the wait (web/CONTRIBUTING.md, \"Writing a spec\"), never rerun past it." >&2
    exit "$status"
fi
echo "No failures in $repeat run(s) of each changed spec."
