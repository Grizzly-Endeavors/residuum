#!/usr/bin/env bash
# Runs a command while the official Playwright container serves a browser, then
# stops the container. The command reaches the browser through E2E_CONTAINER_WS,
# which web/playwright.config.ts gives to the visual-* and webkit-phone
# projects, so visual screenshots render in the same environment on every
# machine, and WebKit needs no system libraries here:
#
#   scripts/with-playwright-container.sh npm --prefix web run e2e:visual
#
# With E2E_ALL_IN_CONTAINER=1 the config gives that browser to the desktop,
# phone and preview-* projects too, so the machine needs no Chromium of its
# own. The CI runners are such machines, and run the whole suite this way:
#
#   E2E_ALL_IN_CONTAINER=1 scripts/with-playwright-container.sh npm --prefix web run e2e:fast
#
# The image tag is the installed Playwright's version, so the browser builds in
# the image are the ones that version expects. The test runner, the mock servers
# and the baselines' comparison stay on this machine, and the container's
# browser reaches the mock through Playwright's `exposeNetwork` tunnel. The
# container mounts the installed playwright-core read-only to run its browser
# server, so both ends are the same version by construction.
#
# Baselines are made for linux/amd64 (the image is pulled for that platform on
# any host), because rendering can differ between architectures.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [args...]" >&2
    exit 2
fi

core=web/node_modules/playwright-core
if [ ! -f "$core/package.json" ]; then
    echo "error: web/node_modules has no Playwright. Run 'just web-install' first." >&2
    exit 1
fi
version="$(node -p "require('./$core/package.json').version")"
image="mcr.microsoft.com/playwright:v${version}-noble"

if [ "${E2E_ALL_IN_CONTAINER:-}" = "1" ]; then
    needs="E2E_ALL_IN_CONTAINER=1 runs every spec in the Playwright container."
    without="To use this machine's Chromium instead, run 'just web-e2e-fast'."
else
    needs="Visual comparisons render in the Playwright container."
    without="The other specs don't need it: run 'just web-e2e-fast'."
fi
if ! command -v docker >/dev/null 2>&1; then
    echo "error: Docker is not installed. $needs" >&2
    echo "       $without" >&2
    exit 1
fi
if ! docker info >/dev/null 2>&1; then
    echo "error: Docker is installed but its daemon can't be reached. $needs" >&2
    echo "       $without" >&2
    exit 1
fi

name="residuum-e2e-playwright-$$"
stop_container() {
    docker rm -f "$name" >/dev/null 2>&1 || true
}
trap stop_container EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "Starting $image (the first run pulls it)..." >&2
# The port is published on loopback only, on whichever host port is free, so
# two runs at once don't collide.
docker run --detach --rm --init --ipc=host --platform linux/amd64 --name "$name" \
    --publish 127.0.0.1::3000 \
    --volume "$PWD/$core:/opt/playwright-core:ro" \
    "$image" \
    node /opt/playwright-core/cli.js run-server --port 3000 --host 0.0.0.0 >/dev/null

for _ in $(seq 1 60); do
    if docker logs "$name" 2>&1 | grep -q '^Listening on '; then
        break
    fi
    if [ -z "$(docker ps --quiet --filter "name=$name")" ]; then
        echo "error: the Playwright container stopped before it was ready:" >&2
        docker logs "$name" >&2 || true
        exit 1
    fi
    sleep 1
done
if ! docker logs "$name" 2>&1 | grep -q '^Listening on '; then
    echo "error: the Playwright container wasn't ready after 60 seconds:" >&2
    docker logs "$name" >&2 || true
    exit 1
fi

host_port="$(docker port "$name" 3000/tcp | head -n 1 | sed 's/.*://')"
export E2E_CONTAINER_WS="ws://127.0.0.1:${host_port}/"

# Not `exec`: the trap has to outlive the command to remove the container.
"$@"
