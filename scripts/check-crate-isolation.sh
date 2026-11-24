#!/usr/bin/env bash
set -uo pipefail

has_test_support_feature() {
    awk '
        /^[ \t]*\[/ {
            section = $0
            next
        }
        section ~ /^[ \t]*\[features\][ \t]*$/ && $0 ~ /^[ \t]*test-support[ \t]*=/ {
            found = 1
        }
        END {
            exit(found ? 0 : 1)
        }
    ' "$1"
}

status=0
members=0

run_check() {
    if ! "$@"; then
        echo "check-crate-isolation: failed: $*" >&2
        status=1
    fi
}

echo "check-crate-isolation: residuum (root library and binary)"
run_check cargo check --quiet -p residuum --lib --bins

for manifest in crates/*/Cargo.toml; do
    [ -f "$manifest" ] || continue
    dir="${manifest#crates/}"
    dir="${dir%/Cargo.toml}"
    package="residuum-$dir"
    members=$((members + 1))
    echo "check-crate-isolation: $package"
    run_check cargo check --quiet -p "$package" --lib
    run_check cargo check --quiet -p "$package" --all-targets
    if has_test_support_feature "$manifest"; then
        run_check cargo check --quiet -p "$package" --lib --features test-support
    fi
done

if [ "$status" -ne 0 ]; then
    echo "check-crate-isolation: a crate does not compile on its own; see the failed commands above" >&2
    exit 1
fi
if [ "$members" -eq 0 ]; then
    echo "check-crate-isolation: no workspace members under crates/; the root library and binary compile on their own"
    exit 0
fi
echo "check-crate-isolation: the root library, the binary and every member compile on their own"
