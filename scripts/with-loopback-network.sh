#!/usr/bin/env bash
# Runs an end-to-end command in a private network that has only loopback, so this
# machine's Chromium never sees the host's interfaces change:
#
#   scripts/with-loopback-network.sh npm --prefix web run e2e:fast
#
# Chromium on Linux watches the host's interfaces and addresses, and when one
# appears or goes away (Docker adds a veth to its bridge for every container it
# starts, and removes it when the container stops) it aborts the page loads in
# flight with net::ERR_NETWORK_CHANGED, loads to 127.0.0.1 included. On a machine
# where other work starts and stops containers, that fails specs at random. Inside
# a network namespace of its own the browser sees only `lo`, which never changes.
# Nothing in the suite needs more: the mock servers listen on 127.0.0.1 and the
# fixtures abort any request that leaves loopback (web/e2e/support/fixtures.ts).
#
# The namespace is unprivileged: `unshare` makes a user namespace and a network
# namespace, brings `lo` up as the namespace's root, then a nested user namespace
# maps the command back to your own uid and gid, so files it writes and the tools
# it runs see you, not root. Needs Linux, util-linux 2.38 or later (`unshare
# --map-user`), iproute2 (`ip`) and a kernel that lets unprivileged users make
# user namespaces. Where any of that is missing (macOS, or a distribution that
# restricts user namespaces), or E2E_HOST_NETWORK=1 is set, the command runs on
# the host's network and one line on stderr says why.
#
# When E2E_CONTAINER_WS names the Playwright container's browser server
# (ws://127.0.0.1:<port>/, set by scripts/with-playwright-container.sh), that
# port is on the host's loopback, which the namespace can't reach. A small Node
# bridge then listens on a Unix socket, which reaches across network namespaces,
# and forwards to the port, and the command gets E2E_CONTAINER_WS as a
# ws+unix:// address on that socket.
set -euo pipefail

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [args...]" >&2
    exit 2
fi

# Inside the new namespaces, as their root: bring loopback up, then drop back to
# the caller's uid and gid. The probe below runs this same path with `true`.
if [ "$1" = "--inside" ]; then
    uid="$2"
    gid="$3"
    shift 3
    ip link set lo up
    exec unshare --user --map-user="$uid" --map-group="$gid" -- "$@"
fi

script="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"

# on_host_network <reason> <command...>
on_host_network() {
    echo "note: the end-to-end browser runs on this machine's network, so a host network change (such as a Docker container starting or stopping) can abort its page loads with ERR_NETWORK_CHANGED: $1." >&2
    shift
    exec "$@"
}

# Prints why the namespace can't be made, or nothing when it can.
namespace_problem() {
    if [ "$(uname -s)" != "Linux" ]; then
        echo "network namespaces are Linux only, and this is $(uname -s)"
        return
    fi
    if ! command -v unshare >/dev/null 2>&1; then
        echo "unshare is not installed (util-linux)"
        return
    fi
    if ! command -v ip >/dev/null 2>&1; then
        echo "ip is not installed (iproute2)"
        return
    fi
    local err
    if ! err="$(unshare --user --map-root-user --net -- "$script" --inside "$(id -u)" "$(id -g)" true 2>&1)"; then
        echo "unshare could not make a loopback-only network ($(printf '%s' "$err" | tr '\n' ' ' | sed 's/ *$//'))"
    fi
}

if [ "${E2E_HOST_NETWORK:-}" = "1" ]; then
    reason="E2E_HOST_NETWORK=1 is set"
else
    reason="$(namespace_problem)"
fi
if [ -n "$reason" ]; then
    on_host_network "$reason" "$@"
fi

enter=(unshare --user --map-root-user --net -- "$script" --inside "$(id -u)" "$(id -g)")

container_ws="${E2E_CONTAINER_WS:-}"
if [ -z "$container_ws" ]; then
    exec "${enter[@]}" "$@"
fi
if ! [[ "$container_ws" =~ ^ws://127\.0\.0\.1:([0-9]+)/?$ ]]; then
    on_host_network "E2E_CONTAINER_WS ($container_ws) is not a port on 127.0.0.1, so the loopback-only network has no way to reach it" "$@"
fi
container_port="${BASH_REMATCH[1]}"

bridge_dir="$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/residuum-e2e.XXXXXX")"
socket="$bridge_dir/browser.sock"
bridge_pid=""
stop_bridge() {
    if [ -n "$bridge_pid" ]; then
        kill "$bridge_pid" 2>/dev/null || true
        wait "$bridge_pid" 2>/dev/null || true
    fi
    rm -rf "$bridge_dir"
}
trap stop_bridge EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Each connection to the socket gets its own connection to the container's port;
# either side closing or failing closes the other.
node -e '
const net = require("node:net");
const [socket, port] = process.argv.slice(1);
net
  .createServer((client) => {
    const upstream = net.connect(Number(port), "127.0.0.1");
    client.pipe(upstream);
    upstream.pipe(client);
    client.on("error", () => upstream.destroy());
    upstream.on("error", (err) => {
      console.error("error: the bridge to the Playwright container (127.0.0.1:" + port + ") failed: " + err.message);
      client.destroy();
    });
    client.on("close", () => upstream.destroy());
    upstream.on("close", () => client.destroy());
  })
  .listen(socket);
' "$socket" "$container_port" &
bridge_pid=$!

for _ in $(seq 1 100); do
    if [ -S "$socket" ]; then
        break
    fi
    if ! kill -0 "$bridge_pid" 2>/dev/null; then
        echo "error: the bridge from the loopback-only network to the Playwright container stopped before it was listening." >&2
        exit 1
    fi
    sleep 0.1
done
if [ ! -S "$socket" ]; then
    echo "error: the bridge from the loopback-only network to the Playwright container wasn't listening on $socket after 10 seconds." >&2
    exit 1
fi

# Not `exec`: the trap has to outlive the command to stop the bridge.
E2E_CONTAINER_WS="ws+unix://$socket:/" "${enter[@]}" "$@"
