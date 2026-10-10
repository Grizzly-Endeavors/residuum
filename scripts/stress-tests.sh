#!/usr/bin/env bash
# Runs the Rust test suite under load, the way it runs when several agents
# build and test on one machine: several copies of every test binary at once,
# with CPU burners alongside. A test that passes here waits on events, not on
# how fast the machine is. `just stress` runs this locally.
#
# Arguments are passed to every test binary, so a libtest filter narrows the
# run: `just stress hub::` or `scripts/stress-tests.sh hub::host::tests`.
#
# Environment:
#   STRESS_COPIES  copies of the suite run at once (default 2)
#   STRESS_ROUNDS  times the whole run repeats (default 1)
#   STRESS_BURN    CPU burners alongside (default: the number of CPUs)
#
# Each copy's output is kept under target/stress/<round>/<copy>.log, and the
# table of failures under target/stress/summary.txt. Fix a failure by making
# the test wait on the event it checks (CONTRIBUTING.md, "Waiting in tests"),
# never by widening a timeout.
set -euo pipefail

copies="${STRESS_COPIES:-2}"
rounds="${STRESS_ROUNDS:-1}"
burn="${STRESS_BURN:-$(nproc)}"
out=target/stress

command -v jq >/dev/null || { echo "error: jq is required to read cargo's build output; install it and run again." >&2; exit 1; }

# ts_export writes web/src/lib/generated, so concurrent copies would race on
# the working tree. It has no timing to stress.
mapfile -t binaries < <(
    cargo test --no-run --message-format=json \
        | jq -r 'select(.reason == "compiler-artifact" and .profile.test == true and .executable != null and .target.name != "ts_export") | .executable' \
        | sort -u
)
if [ "${#binaries[@]}" -eq 0 ]; then
    echo "error: cargo built no test binaries; run 'cargo test --no-run' to see why." >&2
    exit 1
fi

inotify_in_use() {
    # Other users' processes can't be read; find reports that and exits 1.
    { find /proc/[0-9]*/fd -lname 'anon_inode:inotify' 2>/dev/null || true; } | wc -l
}

burners=()
stop_burners() {
    [ "${#burners[@]}" -gt 0 ] && kill "${burners[@]}" 2>/dev/null || true
    burners=()
}
trap stop_burners EXIT INT TERM

# libtest runs tests from the package root and reads its manifest dir at run
# time in a few places; match what `cargo test` sets.
export CARGO_MANIFEST_DIR="$PWD"

rm -rf "$out"
mkdir -p "$out"

echo "Stress: ${#binaries[@]} test binaries, $copies copies at once, $burn CPU burners, $rounds round(s), on $(nproc) CPUs."
echo "Load before: $(cut -d' ' -f1-3 /proc/loadavg)"

for round in $(seq 1 "$rounds"); do
    mkdir -p "$out/$round"
    for _ in $(seq 1 "$burn"); do
        yes >/dev/null &
        burners+=("$!")
    done
    pids=()
    for copy in $(seq 1 "$copies"); do
        (
            status=0
            for binary in "${binaries[@]}"; do
                echo "=== $(basename "$binary")"
                "$binary" --quiet "$@" || {
                    code=$?
                    echo "=== $(basename "$binary") exited $code"
                    status=1
                }
            done
            exit "$status"
        ) >"$out/$round/$copy.log" 2>&1 &
        pids+=("$!")
    done
    inotify_peak="$(inotify_in_use)"
    for pid in "${pids[@]}"; do
        wait "$pid" || true
    done
    stop_burners
    echo "Round $round done. Load: $(cut -d' ' -f1-3 /proc/loadavg). inotify instances in use mid-run: $inotify_peak of $(cat /proc/sys/fs/inotify/max_user_instances 2>/dev/null || echo '?')."
done

# A failing test is named in a "---- <name> stdout ----" header. A binary that
# exited non-zero without naming one crashed or was killed, and counts too.
{
    for log in "$out"/*/*.log; do
        sed -n 's/^---- \(.*\) stdout ----$/\1/p' "$log"
        awk '
            /^=== [^ ]+$/ { bin = $2; named = 0; next }
            /^---- .* stdout ----$/ { named = 1; next }
            /^=== [^ ]+ exited [0-9]+$/ && !named { print bin " (exited " $4 " without naming a failed test)" }
        ' "$log"
    done
} | sort | uniq -c | sort -rn >"$out/summary.txt"

runs=$((copies * rounds))
if [ -s "$out/summary.txt" ]; then
    echo
    echo "Failures (count out of $runs runs of each test):"
    cat "$out/summary.txt"
    echo
    echo "error: tests failed under load. Logs are in $out/; make each test wait on the event it checks (CONTRIBUTING.md, \"Waiting in tests\")." >&2
    exit 1
fi
echo "No failures in $runs runs of each test."
