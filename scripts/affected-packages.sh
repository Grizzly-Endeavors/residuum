#!/usr/bin/env bash
#
# Chooses the cargo packages that a set of changed paths touches. The changed
# paths arrive on stdin, one per line, relative to the working directory, which
# is the repository root; the workspace is read from that same directory.
#
#   --affected (default)  the packages the paths touch, plus every package that
#                         depends on one of them, directly or not, through normal,
#                         dev or build dependencies
#   --members             the member packages among those (never residuum)
#   --graph               the workspace dependency graph, one "package: deps" line each
#
# stdout is the cargo package arguments (`-p residuum-x -p residuum`, or
# `--workspace` when a path touches every package). stderr says which package was
# chosen and why. A manifest that cannot be read falls back to every package (for
# --members, every member) with a warning, so a failure checks more, never less.
# --graph has no fallback: it exits non-zero, because its output is compared with
# the manifests.
#
# Bash 3.2 compatible, because the pre-commit hook runs it on developers' machines
# (no mapfile, no associative arrays). The graph is read with awk, not cargo, so
# the script runs on CI's light runners.
set -euo pipefail
export LC_ALL=C

mode="--affected"
case "${1:-}" in
"") ;;
--affected | --members | --graph) mode="$1" ;;
*)
    echo "usage: affected-packages.sh [--affected | --members | --graph] < changed-paths" >&2
    exit 2
    ;;
esac

# The directory names under crates/ that hold a Cargo.toml, one per line.
member_dirs() {
    local manifest
    for manifest in crates/*/Cargo.toml; do
        [ -f "$manifest" ] || continue
        manifest="${manifest#crates/}"
        printf '%s\n' "${manifest%/Cargo.toml}"
    done
}

# The cargo package arguments for every member, `-p residuum-a -p residuum-b`.
member_args() {
    local dir out=""
    for dir in $(member_dirs); do
        out="${out:+$out }-p residuum-$dir"
    done
    printf '%s\n' "$out"
}

# One line per workspace package and one per edge to another workspace package,
# read from the dependency tables of the root manifest and each member's.
# Node names come from the file: crates/<dir>/Cargo.toml is residuum-<dir>, and
# the root is residuum. The graph's edges are the lines whose key starts with
# residuum-, in [dependencies], [dev-dependencies] and [build-dependencies], and
# in the target-specific forms of those tables.
# shellcheck disable=SC2016 # the awk program's $ names fields, not shell variables
GRAPH_EDGES='
FNR == 1 {
    count = split(FILENAME, parts, "/")
    node = (count == 3) ? "residuum-" parts[2] : "residuum"
    print node
    section = ""
}
{
    sub(/#.*/, "")
    sub(/[ \t]+$/, "")
    if ($0 ~ /^[ \t]*\[/) {
        section = $0
        sub(/^[ \t]*/, "", section)
        next
    }
    if (section !~ /^\[(target\..*\.)?(dependencies|dev-dependencies|build-dependencies)\]$/) next
    if ($0 ~ /^[ \t]*residuum-[A-Za-z0-9_-]+[ \t]*=/) {
        dep = $0
        sub(/^[ \t]*/, "", dep)
        sub(/[ \t]*=.*/, "", dep)
        if (dep != node) print node, dep
    }
}
'

# Sorted "node" and "node dep" lines, grouped into "node: dep dep" lines.
# shellcheck disable=SC2016 # the awk program's $ names fields, not shell variables
GRAPH_GROUP='
{
    if ($1 != current) {
        if (current != "") print line
        current = $1
        line = $1 ":"
    }
    if (NF == 2) line = line " " $2
}
END {
    if (current != "") print line
}
'

# Prints the graph. Fails, naming the file, when a manifest cannot be read.
read_graph() {
    local manifest dir
    local -a manifests=(Cargo.toml)
    for dir in $(member_dirs); do
        manifests+=("crates/$dir/Cargo.toml")
    done
    for manifest in "${manifests[@]}"; do
        if [ ! -r "$manifest" ]; then
            echo "affected-packages: cannot read $manifest" >&2
            return 1
        fi
    done
    awk "$GRAPH_EDGES" "${manifests[@]}" | sort -u | awk "$GRAPH_GROUP"
}

if [ "$mode" = "--graph" ]; then
    if ! graph="$(read_graph)"; then
        echo "affected-packages: the workspace manifests cannot be read, so the graph cannot be printed" >&2
        exit 1
    fi
    printf '%s\n' "$graph"
    exit 0
fi

graph_ok=1
graph="$(read_graph)" || graph_ok=0
if [ "$graph_ok" -eq 0 ]; then
    echo "warning: falling back to every package, because a workspace manifest cannot be read" >&2
    if [ "$mode" = "--members" ]; then
        member_args
    else
        echo "--workspace"
    fi
    exit 0
fi

# Sets CLASSIFIED to the package a changed path touches, to ALL when it touches
# every package, or to nothing when no package builds from it.
classify_path() {
    local path="$1" rest dir
    CLASSIFIED=""
    case "$path" in
    crates/*)
        rest="${path#crates/}"
        dir="${rest%%/*}"
        if [ "$rest" != "$dir" ] && [ -f "crates/$dir/Cargo.toml" ]; then
            CLASSIFIED="residuum-$dir"
        else
            CLASSIFIED="ALL"
        fi
        ;;
    tests/fixtures/* | assets/* | .cargo/* | Cargo.toml | Cargo.lock | rust-toolchain.toml | clippy.toml | rustfmt.toml | deny.toml | .github/workflows/ci.yml | .github/workflows/rust-checks.yml | scripts/affected-packages* | scripts/check-crate-isolation.sh)
        CLASSIFIED="ALL"
        ;;
    src/* | tests/* | build.rs | web/src/lib/generated/* | web/app-shell-csp.txt | scripts/check-generated-types.sh | scripts/export-generated-types.sh | .github/workflows/release.yml)
        CLASSIFIED="residuum"
        ;;
    web/src/styles/tokens.css)
        CLASSIFIED="residuum-net"
        ;;
    esac
    return 0
}

# The selected packages, space-separated and padded, so membership is a substring test.
names=" "
has() {
    case "$names" in
    *" $1 "*) return 0 ;;
    esac
    return 1
}
add() {
    names="$names$1 "
    echo "$1 ($2)" >&2
}

all_path=""
while IFS= read -r path; do
    [ -n "$path" ] || continue
    classify_path "$path"
    case "$CLASSIFIED" in
    "") ;;
    ALL)
        if [ -z "$all_path" ]; then all_path="$path"; fi
        ;;
    *)
        if ! has "$CLASSIFIED"; then add "$CLASSIFIED" "changed: $path"; fi
        ;;
    esac
done

if [ -n "$all_path" ]; then
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        add "${line%%:*}" "changed: $all_path"
    done <<<"$graph"
else
    pending=1
    while [ "$pending" -eq 1 ]; do
        pending=0
        while IFS= read -r line; do
            [ -n "$line" ] || continue
            pkg="${line%%:*}"
            if has "$pkg"; then continue; fi
            for dep in ${line#*:}; do
                if has "$dep"; then
                    add "$pkg" "depends on $dep"
                    pending=1
                    break
                fi
            done
        done <<<"$graph"
    done
fi

if [ "$mode" = "--members" ] && [ -n "$all_path" ]; then
    member_args
    exit 0
fi

if [ -n "$all_path" ]; then
    echo "--workspace"
    exit 0
fi

out=""
while IFS= read -r line; do
    [ -n "$line" ] || continue
    pkg="${line%%:*}"
    if [ "$mode" = "--members" ] && [ "$pkg" = "residuum" ]; then continue; fi
    if has "$pkg"; then
        out="${out:+$out }-p $pkg"
    fi
done <<<"$graph"
printf '%s\n' "$out"
