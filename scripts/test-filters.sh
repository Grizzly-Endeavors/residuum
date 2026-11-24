#!/usr/bin/env bash
set -eo pipefail

usage() {
    echo "usage: $0 note <test|stress> [args...]" >&2
    echo "       $0 check <test|stress> <log>... -- [args...]" >&2
    exit 2
}

cargo_value_opts=" -p --package --test --bin --example --bench --exclude -F --features --target --target-dir --manifest-path --profile -j --jobs --color --message-format --config -Z "

filters_of() {
    local kind="$1"
    shift
    case $kind in
        stress)
            local leading=1 seen_package=0 skip=0 arg
            for arg in "$@"; do
                if [ "$skip" -eq 1 ]; then
                    skip=0
                    continue
                fi
                if [ "$leading" -eq 1 ]; then
                    case $arg in
                        -p | --package)
                            seen_package=1
                            skip=1
                            continue
                            ;;
                        --package=*)
                            seen_package=1
                            continue
                            ;;
                        --)
                            if [ "$seen_package" -eq 1 ]; then
                                leading=0
                                continue
                            fi
                            ;;
                    esac
                    leading=0
                fi
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

it_modules_covering() { # crate name
    local name="$1" file rest module covers
    for file in tests/it/*.rs tests/it/*/mod.rs; do
        [ -f "$file" ] || continue
        rest=${file#tests/it/}
        module=${rest%%/*}
        module=${module%.rs}
        if [ "$module" = main ]; then
            continue
        fi
        covers="$(sed -n 's|^//! covers: ||p' "$file" | head -n 1)"
        covers=${covers// /}
        case ",$covers," in
            *",residuum-$name,"*)
                echo "note: $file covers residuum-$name; run cargo test --quiet --test it $module::" >&2
                ;;
        esac
    done
    return 0
}

note_for() {
    local filter="$1" mode="$2" seg rest sp kind name
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
        if [ "$mode" = stress ]; then
            case $kind in
                dir)
                    echo "note: $seg is crate directory crates/$name (residuum-$name), whose test paths start below the crate; run just stress -p residuum-$name$sp$rest" >&2
                    ;;
                module)
                    echo "note: $seg is a module of residuum-$name, so its tests are that crate's; run just stress -p residuum-$name $filter" >&2
                    ;;
            esac
            continue
        fi
        case $kind in
            dir)
                echo "note: $seg is crate directory crates/$name (residuum-$name), whose test paths start below the crate; run just test-crate $name$sp$rest or just test -p residuum-$name$sp$rest" >&2
                ;;
            module)
                echo "note: $seg is a module of residuum-$name, so its tests are that crate's; run just test-crate $name $filter or just test -p residuum-$name $filter" >&2
                ;;
        esac
        it_modules_covering "$name"
    done
    return 0
}

cmd_note() {
    local kind="$1" arg
    shift
    if [ "$kind" = stress ]; then
        for arg in "$@"; do
            case $arg in
                -p | --package | --package=*) return 0 ;;
            esac
        done
    fi
    filters_of "$kind" "$@" | while IFS= read -r filter; do
        note_for "$filter" "$kind"
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
            n="$(awk '/^running [0-9]+ tests?$/ { sum += $2 } END { print sum + 0 }' "$log")"
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
