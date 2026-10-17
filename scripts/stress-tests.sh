#!/usr/bin/env bash
# Runs the Rust test suite under load, the way it runs when several agents
# build and test on one machine: several copies of every test binary at once,
# with CPU burners alongside. A test that passes here waits on events, not on
# how fast the machine is. `just stress` runs this locally.
#
# Leading `-p <package>` (or `--package`) options choose the packages whose test
# binaries are built and run, so a member crate's tests can be stressed:
# `just stress -p residuum-core wait::`. Without them every package in
# default-members is built. Every argument after them is passed to each test
# binary, so a libtest filter narrows the run: `just stress gateway::` or
# `scripts/stress-tests.sh hub_host::`.
#
# Environment:
#   STRESS_COPIES  copies of the suite run at once (default 2)
#   STRESS_ROUNDS  times the whole run repeats (default 1)
#   STRESS_BURN    CPU burners alongside (default: the number of CPUs)
#   STRESS_TESTS_LOG  output of the unloaded `cargo test --no-fail-fast` run.
#                  When set, the failures are split by whether that run also
#                  failed them (see below).
#
# Each copy's output is kept under target/stress/<round>/<copy>.log, and the
# failures under target/stress/summary.txt. A failure that failed only under
# load is a test waiting on time: fix it by making the test wait on the event
# it checks (CONTRIBUTING.md, "Waiting in tests"), never by widening a timeout.
# A failure that also failed in the unloaded run is an ordinary failure.
set -euo pipefail

copies="${STRESS_COPIES:-2}"
rounds="${STRESS_ROUNDS:-1}"
burn="${STRESS_BURN:-$(nproc)}"
out=target/stress

command -v jq >/dev/null || { echo "error: jq is required to read cargo's build output; install it and run again." >&2; exit 1; }
if [ -n "${STRESS_TESTS_LOG:-}" ] && [ ! -f "$STRESS_TESTS_LOG" ]; then
    echo "error: STRESS_TESTS_LOG is set to $STRESS_TESTS_LOG, which is not a file." >&2
    exit 1
fi

# Clears the previous run's logs before the build, so a run that fails to build
# cannot leave them for the caller's summary to count.
rm -rf "$out"

# Leading package options, then the libtest arguments. A `--` after a package
# option ends the options; without one, `--` is a libtest argument like any other.
cargo_args=()
while [ "$#" -gt 0 ]; do
    case $1 in
        -p | --package)
            if [ "$#" -lt 2 ]; then
                echo "error: $1 needs a package name" >&2
                exit 2
            fi
            cargo_args+=(-p "$2")
            shift 2
            ;;
        --package=*)
            cargo_args+=(-p "${1#--package=}")
            shift
            ;;
        --)
            if [ "${#cargo_args[@]}" -gt 0 ]; then
                shift
            fi
            break
            ;;
        *) break ;;
    esac
done

# ts_export writes web/src/lib/generated, so concurrent copies would race on
# the working tree. It has no timing to stress. Each line is a binary's path and
# its package's directory, tab-separated.
mapfile -t binary_entries < <(
    cargo test --no-run --message-format=json "${cargo_args[@]}" \
        | jq -r 'select(.reason == "compiler-artifact" and .profile.test == true and .executable != null and .target.name != "ts_export") | [.executable, (.manifest_path | rtrimstr("/Cargo.toml"))] | @tsv' \
        | sort -u
)
binaries=()
manifest_dirs=()
for entry in "${binary_entries[@]}"; do
    binaries+=("${entry%%$'\t'*}")
    manifest_dirs+=("${entry#*$'\t'}")
done
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
    if [ "${#burners[@]}" -gt 0 ]; then
        kill "${burners[@]}" 2>/dev/null || true
    fi
    burners=()
}
trap stop_burners EXIT INT TERM

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
            # Each binary runs from its package's directory with CARGO_MANIFEST_DIR
            # set to it, as `cargo test` does: some tests read paths relative to it.
            for index in "${!binaries[@]}"; do
                binary="${binaries[$index]}"
                dir="${manifest_dirs[$index]}"
                echo "=== $(basename "$binary")"
                (cd "$dir" && CARGO_MANIFEST_DIR="$dir" "$binary" --quiet "$@") || {
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

# A failing test is named in a "---- <name> stdout ----" header. A test in a
# member crate's binary (residuum_<x>-<hash>) is listed with its crate first,
# residuum_<x>::<name>. A binary that exited non-zero without naming one crashed
# or was killed, and counts too.
{
    awk '
        FNR == 1 { bin = ""; named = 0 }
        /^=== [^ ]+$/ { bin = $2; named = 0; next }
        /^---- .* stdout ----$/ {
            name = $0
            sub(/^---- /, "", name)
            sub(/ stdout ----$/, "", name)
            if (bin ~ /^residuum_[a-z0-9_]+-[0-9a-f]+$/) {
                crate = bin
                sub(/-[0-9a-f]+$/, "", crate)
                name = crate "::" name
            }
            print name
            named = 1
            next
        }
        /^=== [^ ]+ exited [0-9]+$/ && !named { print bin " (exited " $4 " without naming a failed test)" }
    ' "$out"/*/*.log
} | sort | uniq -c | sort -rn >"$out/counts.txt"

# Writes one heading and its lines, or nothing when there are none.
section() {
    [ -n "$2" ] || return 0
    echo "$1"
    printf '%s' "$2"
    echo
}

runs=$((copies * rounds))
if [ -s "$out/counts.txt" ]; then
    if [ -n "${STRESS_TESTS_LOG:-}" ]; then
        advice="the summary says which failures waited on time and which also failed in the Tests step"
        tests_failed="$(sed -n 's/^---- \(.*\) stdout ----$/\1/p' "$STRESS_TESTS_LOG" | sort -u)"
        load_only="" both="" crashed=""
        while read -r count name; do
            line="$count $name"$'\n'
            # The Tests step's log names a test by its path inside its crate, so the
            # crate is dropped before the comparison.
            bare="$name"
            if [[ "$name" =~ ^residuum_[a-z0-9_]+:: ]]; then
                bare="${name#*::}"
            fi
            if [[ "$name" == *"(exited "* ]]; then
                crashed+="$line"
            elif grep -Fxq -- "$bare" <<<"$tests_failed"; then
                both+="$line"
            else
                load_only+="$line"
            fi
        done <"$out/counts.txt"
        {
            section "Failed under load only, so a test waiting on time (fix the wait; CONTRIBUTING.md, \"Waiting in tests\"):" "$load_only"
            section "Also failed in the Tests step, an ordinary failure (see the Tests step):" "$both"
            section "Exited without naming a failed test (see the Tests step and the copy logs):" "$crashed"
        } >"$out/summary.txt"
    else
        advice="make each test wait on the event it checks (CONTRIBUTING.md, \"Waiting in tests\")"
        cp "$out/counts.txt" "$out/summary.txt"
    fi
    echo
    echo "Failures (count out of $runs runs of each test):"
    cat "$out/summary.txt"
    echo
    echo "error: tests failed under load. Logs are in $out/; $advice." >&2
    exit 1
fi
echo "No failures in $runs runs of each test."
