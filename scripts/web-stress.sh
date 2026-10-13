#!/usr/bin/env bash
# Runs the end-to-end specs that a branch adds or changes, repeatedly and under
# CPU load: the web counterpart of scripts/stress-tests.sh. A race in a new spec
# then fails on the pull request that adds it, instead of flaking on some later,
# unrelated pull request. CI's Web Stress job runs it on pull requests
# (.github/workflows/quality-checks.yml), and `just web-stress` runs it here.
#
# Which specs run is decided by Playwright's --only-changed against the merge base
# with the base branch. That follows each spec's import graph, so a change to a
# shared helper selects every spec that imports it. When no spec is selected the
# script says so and passes.
#
# Arguments go to `playwright test` after the projects, so a file or a --grep
# narrows the run: `just web-stress --grep "tab title"`.
#
# Environment:
#   WEB_STRESS_BASE     branch whose merge base the changes are taken against (default origin/main)
#   WEB_STRESS_REPEAT   times each selected test runs (default 5)
#   WEB_STRESS_WORKERS  workers, as E2E_WORKERS (default 2). Playwright's --workers
#                       flag is not used: each worker starts its own mock servers, and
#                       the config reads E2E_WORKERS to start them.
#   WEB_STRESS_BURN     CPU burners alongside the run: a number, or `auto` for one per
#                       CPU (default 0, no load). CI sets `auto`, as the Rust Stress
#                       step does, so a run there shares its CPUs the way a busy
#                       machine does.
#
# The projects are the ones `npm run e2e:fast` runs, minus the visual projects,
# which the visual comparisons cover, and webkit-phone, which is local only.
#
# A failure is listed in Playwright's list output, which scripts/ci-failures.sh
# reads. A spec that fails only here is waiting on time instead of the event it
# checks: fix the wait (web/CONTRIBUTING.md, "Writing a spec"), never rerun past it.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
base_ref="${WEB_STRESS_BASE:-origin/main}"
repeat="${WEB_STRESS_REPEAT:-5}"
workers="${WEB_STRESS_WORKERS:-2}"
burn="${WEB_STRESS_BURN:-0}"

if ! [[ "$repeat" =~ ^[1-9][0-9]*$ ]]; then
    echo "error: WEB_STRESS_REPEAT must be a whole number of at least 1, got \"$repeat\"." >&2
    exit 1
fi
if ! [[ "$workers" =~ ^[1-9][0-9]*$ ]]; then
    echo "error: WEB_STRESS_WORKERS must be a whole number of at least 1, got \"$workers\"." >&2
    exit 1
fi
if [ "$burn" = auto ]; then
    burn="$(nproc)"
fi
if ! [[ "$burn" =~ ^[0-9]+$ ]]; then
    echo "error: WEB_STRESS_BURN must be a number or \"auto\", got \"$burn\"." >&2
    exit 1
fi

projects=(
    --project=desktop
    --project=phone
    --project=dev-desktop
    --project=dev-phone
    --project=preview-desktop
    --project=preview-phone
)

cd "$repo"
base="$(git merge-base "$base_ref" HEAD)" || {
    echo "error: no merge base with $base_ref. Fetch it (git fetch origin main), or set WEB_STRESS_BASE." >&2
    exit 1
}

cd "$repo/web"

# --list runs no test and starts no server, so it tells whether any spec is
# selected, and which. Its errors (a spec that does not compile) end the script here.
listing="$(npx playwright test --list --only-changed="$base" "${projects[@]}" "$@")"
selected="$(sed -n 's/^  \[[^]]*\] › \([^:]*\):.*$/\1/p' <<<"$listing" | sort -u)"
if [ -z "$selected" ]; then
    echo "No changed specs against $base_ref: nothing to repeat."
    exit 0
fi

tests="$(sed -n 's/^Total: \([0-9]*\) tests\{0,1\} in .*$/\1/p' <<<"$listing")"
mapfile -t specs <<<"$selected"
echo "Changed specs against $base_ref (merge base $base):"
printf '  %s\n' "${specs[@]}"
echo "Stress: $tests test instances, each run $repeat times, $workers workers, $burn CPU burners."
echo "Load before: $(cut -d' ' -f1-3 /proc/loadavg)"

burners=()
stop_burners() {
    if [ "${#burners[@]}" -gt 0 ]; then
        kill "${burners[@]}" 2>/dev/null || true
    fi
    burners=()
}
trap stop_burners EXIT INT TERM

for _ in $(seq 1 "$burn"); do
    yes >/dev/null &
    burners+=("$!")
done

status=0
E2E_WORKERS="$workers" npx playwright test --only-changed="$base" "${projects[@]}" \
    --repeat-each="$repeat" --retries=0 "$@" || status=$?

stop_burners
echo "Load after: $(cut -d' ' -f1-3 /proc/loadavg)"

if [ "$status" -ne 0 ]; then
    echo "error: the stress run failed ($repeat runs each, $workers workers, $burn CPU burners): a spec failed under load, or the run did not start. The failures and the report (web/playwright-report) are above. A spec that fails only under load is waiting on time instead of the event it checks: fix the wait (web/CONTRIBUTING.md, \"Writing a spec\"), never rerun past it." >&2
    exit "$status"
fi
echo "No failures in $repeat runs of each changed spec."
