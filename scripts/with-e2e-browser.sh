#!/usr/bin/env bash
# Runs an end-to-end command with a browser it can drive: this machine's
# Chromium when Playwright can install it, otherwise the Playwright container.
#
#   scripts/with-e2e-browser.sh npm --prefix web run e2e:fast
#   scripts/with-e2e-browser.sh --visual npm --prefix web run e2e
#
# `npx playwright install chromium` is a no-op once the browser is installed. On
# a machine whose route to cdn.playwright.dev stalls it never finishes, so it runs
# under a time limit (E2E_BROWSER_INSTALL_TIMEOUT, in seconds, default 240). When
# the install fails or times out, this script says so, then runs the command through
# scripts/with-playwright-container.sh with E2E_ALL_IN_CONTAINER=1, which sends
# every non-visual project to the container's browser as well. When Docker is
# unavailable too, it fails and names both problems.
#
# --visual marks a command that needs the container even when Chromium installs
# (the visual comparisons render there): the script then starts the container
# itself, with E2E_ALL_IN_CONTAINER=1 only when the install failed. Without it the
# command runs directly when Chromium installs, and inside the container when not.
set -euo pipefail

cd "$(dirname "$0")/.."

visual=0
if [ "${1:-}" = "--visual" ]; then
    visual=1
    shift
fi
if [ "$#" -eq 0 ]; then
    echo "usage: $0 [--visual] <command> [args...]" >&2
    exit 2
fi

wrapper=scripts/with-playwright-container.sh
limit="${E2E_BROWSER_INSTALL_TIMEOUT:-240}"
if ! [[ "$limit" =~ ^[1-9][0-9]*$ ]]; then
    echo "error: E2E_BROWSER_INSTALL_TIMEOUT must be a whole number of seconds, got \"$limit\"." >&2
    exit 1
fi

# --kill-after: a download that ignores the first TERM must not hold the recipe.
install_status=0
(cd web && timeout --kill-after=10 "$limit" npx playwright install chromium) || install_status=$?
if [ "$install_status" -eq 0 ]; then
    if [ "$visual" -eq 1 ]; then
        exec "$wrapper" "$@"
    fi
    exec "$@"
fi

if [ "$install_status" -eq 124 ] || [ "$install_status" -eq 137 ]; then
    reason="timed out after ${limit}s"
else
    reason="failed (status $install_status)"
fi
echo "warning: installing Playwright's Chromium $reason. The download from cdn.playwright.dev may be blocked or stalled on this machine." >&2

docker_problem=""
if ! command -v docker >/dev/null 2>&1; then
    docker_problem="Docker is not installed"
elif ! docker info >/dev/null 2>&1; then
    docker_problem="Docker is installed but its daemon can't be reached"
fi
if [ -n "$docker_problem" ]; then
    echo "error: the end-to-end specs have no browser to run in. Two things are wrong: (1) Playwright's Chromium $reason; (2) $docker_problem, so the Playwright container can't stand in for it." >&2
    echo "       Fix either one: check this machine's route to cdn.playwright.dev and rerun, or install and start Docker (https://docs.docker.com/engine/install/). A fresh Linux machine may also need Chromium's system libraries: cd web && npx playwright install --with-deps chromium" >&2
    exit 1
fi

echo "Switching to the Playwright container for this run: every non-visual project uses its browser (E2E_ALL_IN_CONTAINER=1). The first run pulls the image, about a gigabyte." >&2
E2E_ALL_IN_CONTAINER=1 exec "$wrapper" "$@"
