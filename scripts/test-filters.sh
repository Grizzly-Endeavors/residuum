#!/usr/bin/env bash
# Checks the test filters given to `just test` and `just stress`. A filter whose
# first `::` segment names a member crate, or a module inside one, gets a note
# pointing at `just test-crate`; a run that was given filters and matched no
# test gets a warning. Both go to stderr and neither changes the run's status:
# the recipes call this after cargo or the stress run has finished.
#
# Usage:
#   scripts/test-filters.sh note  <test|stress> [args...]
#   scripts/test-filters.sh check <test|stress> <log>... -- [args...]
#
# `test` takes cargo's arguments, so only those libtest receives are filters;
# `stress` passes every argument to libtest. `check` sums the `running N tests`
# lines of each log.
#
# This runs under bash 3.2, so it avoids arrays and `set -u`.
set -eo pipefail

usage() {
    echo "usage: $0 note <test|stress> [args...]" >&2
    echo "       $0 check <test|stress> <log>... -- [args...]" >&2
    exit 2
}

# cargo test options that take their value as the next argument.
cargo_value_opts=" -p --package --test --bin --example --bench --exclude -F --features --target --target-dir --manifest-path --profile -j --jobs --color --message-format --config -Z "

# Prints the libtest filters among the arguments, one per line.
filters_of() {
    local kind="$1"
    shift
    case $kind in
        stress)
            for arg in "$@"; do
                if [ -n "$arg" ]; then printf '%s\n' "$arg"; fi
            done
            ;;
        test)
            local after_dd=0 skip=0 arg
            for arg in "$@"; do
                if [ "$after_dd" -eq 1 ]; then
                    case $arg in
                        -* | "") ;;
                        *) printf '%s\n' "$arg" ;;
                    esac
                    continue
                fi
                if [ "$skip" -eq 1 ]; then
                    skip=0
                    continue
                fi
                case $arg in
                    --) after_dd=1 ;;
                    -*=*) ;;
                    -*)
                        case $cargo_value_opts in
                            *" $arg "*) skip=1 ;;
                        esac
                        ;;
                    "") ;;
                    *) printf '%s\n' "$arg" ;;
                esac
            done
            ;;
        *) usage ;;
    esac
    return 0
}

# Prints one "dir <name>" line for each crate directory the segment names, and one
# "module <crate>" line for each crate holding a module file or directory of the
# segment's name. A crate directory may be spelled with `-` where the path segment
# has `_` (`tracing-service` is `tracing_service::…`). Names may repeat.
crates_named_by() {
    local seg="$1" dir name
    case $seg in
        "" | *[!A-Za-z0-9_]*) return 0 ;;
    esac
    for name in "$seg" "${seg//_/-}"; do
        if [ -d "crates/$name" ]; then
            printf 'dir %s\n' "$name"
        fi
    done
    for dir in crates/*/src/"$seg".rs crates/*/src/"$seg"/; do
        if [ -e "$dir" ]; then
            dir=${dir#crates/}
            printf 'module %s\n' "${dir%%/*}"
        fi
    done
    return 0
}

note_for() {
    local filter="$1" seg rest sp kind name
    case $filter in
        *::*)
            seg=${filter%%::*}
            rest=${filter#*::}
            ;;
        *)
            seg=$filter
            rest=""
            ;;
    esac
    sp=""
    if [ -n "$rest" ]; then sp=" "; fi
    crates_named_by "$seg" | sort -u | while read -r kind name; do
        case $kind in
            dir)
                echo "note: $seg is crate directory crates/$name (residuum-$name), whose test paths start below the crate; run just test-crate $name$sp$rest or just test -p residuum-$name$sp$rest" >&2
                ;;
            module)
                echo "note: $seg is a module of residuum-$name, so its tests are that crate's; run just test-crate $name $filter or just test -p residuum-$name $filter" >&2
                ;;
        esac
    done
    return 0
}

cmd_note() {
    local kind="$1"
    shift
    filters_of "$kind" "$@" | while IFS= read -r filter; do
        note_for "$filter"
    done
    return 0
}

cmd_check() {
    local kind="$1" total=0 log n filters joined
    shift
    while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
        log=$1
        shift
        if [ -f "$log" ]; then
            # The log may hold colour codes (`just test` keeps cargo's colour), so they go before matching.
            n="$(awk 'BEGIN { esc = sprintf("%c", 27) } { gsub(esc "\\[[0-9;]*m", "") } /^running [0-9]+ tests?$/ { sum += $2 } END { print sum + 0 }' "$log")"
            total=$((total + n))
        fi
    done
    if [ "$#" -gt 0 ]; then
        shift
    fi
    filters="$(filters_of "$kind" "$@")"
    if [ -n "$filters" ] && [ "$total" -eq 0 ]; then
        joined="$(printf '%s' "$filters" | tr '\n' ' ')"
        echo "warning: no test matched ${joined% }" >&2
    fi
    return 0
}

[ "$#" -ge 2 ] || usage
case $1 in
    note)
        shift
        cmd_note "$@"
        ;;
    check)
        shift
        cmd_check "$@"
        ;;
    *) usage ;;
esac
