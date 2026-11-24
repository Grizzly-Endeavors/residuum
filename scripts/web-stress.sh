#!/usr/bin/env bash
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
all="${WEB_STRESS_ALL:-}"
base_ref="${WEB_STRESS_BASE:-origin/main}"
repeat_cap="${WEB_STRESS_REPEAT:-5}"
instance_budget="${WEB_STRESS_BUDGET:-400}"
workers="${WEB_STRESS_WORKERS:-2}"
throttle="${WEB_STRESS_CPU_THROTTLE:-4}"

require_whole() {
    local name="$1" value="$2"
    if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
        echo "error: $name must be a whole number of at least 1, got \"$value\"." >&2
        exit 1
    fi
}
require_whole WEB_STRESS_REPEAT "$repeat_cap"
require_whole WEB_STRESS_BUDGET "$instance_budget"
require_whole WEB_STRESS_WORKERS "$workers"
require_whole WEB_STRESS_CPU_THROTTLE "$throttle"
if [ -n "$all" ] && [ "$all" != 1 ]; then
    echo "error: WEB_STRESS_ALL must be 1 or unset, got \"$all\"." >&2
    exit 1
fi

record() {
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
        echo "summary=$1" >>"$GITHUB_OUTPUT"
    fi
}

cd "$repo"
specs=()
if [ "$all" = 1 ]; then
    echo "Every spec (WEB_STRESS_ALL=1)."
    spec_list="every spec"
else
    base="$(git merge-base "$base_ref" HEAD)" || {
        echo "error: no merge base with $base_ref. Fetch it (git fetch origin main), set WEB_STRESS_BASE, or set WEB_STRESS_ALL=1 to run every spec." >&2
        exit 1
    }

    edited="$(git diff --name-only --diff-filter=d "$base" -- 'web/e2e/**/*.spec.ts' 'web/e2e/*.spec.ts')"
    added="$(git ls-files --others --exclude-standard -- 'web/e2e/**/*.spec.ts' 'web/e2e/*.spec.ts')"
    changed="$(printf '%s\n%s\n' "$edited" "$added" | sed -e '/^$/d' -e 's|^web/||' | sort -u)"

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
    spec_list="$(printf '%s, ' "${specs[@]}" | sed 's/, $//')"
fi

cd "$repo/web"

list_status=0
listing="$(npm run --silent e2e:fast -- --list --pass-with-no-tests "${specs[@]}" "$@" 2>&1)" || list_status=$?
if [ "$list_status" -ne 0 ]; then
    # Playwright names the file that failed as `smoke/x.spec.ts` (relative to web/e2e)
    # or with its absolute path, so match on the part after `e2e/`.
    broken=""
    for spec in "${specs[@]}"; do
        if grep -qF "${spec#e2e/}" <<<"$listing"; then
            broken+="${broken:+, }$spec"
        fi
    done
    printf '%s\n' "$listing" >&2
    echo "error: Playwright could not load the selected specs (status $list_status). Failed to load: ${broken:-not identified, see the output above}. Fix the error the output shows; the step fails until the listing succeeds." >&2
    exit "$list_status"
fi
instances="$(sed -n 's/^Total: \([0-9]*\) tests\{0,1\} in .*$/\1/p' <<<"$listing")"
if ! [[ "$instances" =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$listing" >&2
    echo "error: could not read the test count from Playwright's listing: it has no single \"Total:\" line." >&2
    exit 1
fi
if [ "$instances" -eq 0 ]; then
    echo "The selected specs have no tests in these projects: nothing to repeat."
    record "the selected specs have no tests in these projects"
    exit 0
fi

repeat="$repeat_cap"
lowered=""
if (( instances * repeat > instance_budget )); then
    repeat=$(( instance_budget / instances ))
    (( repeat >= 1 )) || repeat=1
    lowered="; WEB_STRESS_REPEAT=$repeat_cap lowered by the budget of $instance_budget instance-runs"
fi

record "$spec_list ($instances test instances, $repeat run(s) each, CPU throttled ${throttle}x, $workers workers)"

echo "Stress: $instances test instances, $repeat run(s) each$lowered. $workers workers, CPU throttled ${throttle}x."

status=0
E2E_WORKERS="$workers" E2E_CPU_THROTTLE="$throttle" \
    npm run --silent e2e:fast -- --repeat-each="$repeat" --retries=0 "${specs[@]}" "$@" || status=$?

if [ "$status" -ne 0 ]; then
    echo "error: the stress run failed ($repeat run(s) each, $workers workers, CPU throttled ${throttle}x): a spec failed under load, or the run did not start. The failures and the report (web/playwright-report) are above. A spec that fails only under load is waiting on time instead of the event it checks: fix the wait (web/CONTRIBUTING.md, \"Writing a spec\"), never rerun past it." >&2
    exit "$status"
fi
echo "No failures in $repeat run(s) of each selected test."
