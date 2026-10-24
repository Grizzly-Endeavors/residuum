#!/usr/bin/env bash
#
# Chooses the cargo packages that a set of changed paths touches. The changed
# paths arrive on stdin, one per line, relative to the working directory, which
# is the repository root; the workspace is read from that same directory.
#
#   --affected (default)  the packages the paths touch, plus every package that
#                         depends on one of them, directly or not, through normal,
#                         dev or build dependencies
#   --touched             the member packages the paths are in, without dependents
#   --covering            the integration test modules to run for the paths, one per
#                         line: `it:<m>` for tests/it/<m>, and `it:ALL` for a path
#                         every module reads
#   --graph               the workspace dependency graph, one "package: deps" line each
#
# stdout is the cargo package arguments (`-p residuum-x -p residuum`, or
# `--workspace` when a path touches every package). stderr says which package was
# chosen and why. --affected falls back to every package with a warning when a
# manifest cannot be read, so a failure checks more, never less. --touched and
# --covering have no fallback of their own: when a manifest or a covers line
# cannot be read they exit non-zero, and the caller runs every member or every
# module instead. --graph has no fallback either: it exits non-zero, because its
# output is compared with the manifests.
#
# Bash 3.2 compatible, because the pre-commit hook runs it on developers' machines
# (no mapfile, no associative arrays). The graph is read with awk, not cargo, so
# the script runs on CI's light runners.
set -euo pipefail
export LC_ALL=C

mode="--affected"
case "${1:-}" in
"") ;;
--affected | --touched | --covering | --graph) mode="$1" ;;
*)
    echo "usage: affected-packages.sh [--affected | --touched | --covering | --graph] < changed-paths" >&2
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
if [ "$graph_ok" -eq 0 ] && { [ "$mode" = "--touched" ] || [ "$mode" = "--covering" ]; }; then
    echo "affected-packages: the workspace manifests cannot be read, so $mode cannot choose" >&2
    exit 1
fi
if [ "$graph_ok" -eq 0 ]; then
    echo "warning: falling back to every package, because a workspace manifest cannot be read" >&2
    echo "--workspace"
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
    web/app-shell-csp.txt | .github/workflows/release.yml)
        CLASSIFIED="residuum-hub"
        ;;
    src/* | tests/* | build.rs | web/src/lib/generated/* | scripts/check-generated-types.sh | scripts/export-generated-types.sh)
        CLASSIFIED="residuum"
        ;;
    web/src/styles/tokens.css)
        CLASSIFIED="residuum-net"
        ;;
    esac
    return 0
}

# Prints the packages the changed paths in are, one per line, for --touched.
# Only a member's path counts; a root path or a path no package builds from
# prints nothing.
touched_packages() {
    local path
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        classify_path "$path"
        case "$CLASSIFIED" in
        "" | ALL | residuum) ;;
        *) printf '%s\n' "$CLASSIFIED" ;;
        esac
    done | sort -u
}

# Prints one "<label> <package>" line for each name on the covers line of each
# integration module: tests/it/<m>.rs or tests/it/<m>/mod.rs. The container file
# (tests/it/main.rs) and tests/it/support are not modules.
covers_table() {
    local file name
    shopt -s nullglob
    for file in tests/it/*.rs; do
        name="${file##*/}"
        name="${name%.rs}"
        if [ "$name" != main ]; then covers_of "it:$name" "$file"; fi
    done
    for file in tests/it/*/mod.rs; do
        name="${file#tests/it/}"
        name="${name%%/*}"
        if [ "$name" != support ]; then covers_of "it:$name" "$file"; fi
    done
    shopt -u nullglob
}

# Prints "<label> <package>" for each name on the canonical covers line of $2,
# `//! covers: <package>, <package>, …`, read from the leading doc comment. The
# scan in src/testing/source_scan.rs (it_covers) rejects any other spelling.
covers_of() {
    if [ ! -r "$2" ]; then
        echo "affected-packages: cannot read $2, so its covers line is unknown" >&2
        exit 1
    fi
    awk -v label="$1" '
        /^[ \t]*$/ { next }
        /^\/\/!/ {
            if ($0 !~ /^\/\/! covers: /) next
            line = substr($0, length("//! covers: ") + 1)
            count = split(line, names, ", ")
            for (i = 1; i <= count; i++) print label, names[i]
            next
        }
        { exit }
    ' "$2"
}

# Prints the module label a path under tests/it selects directly, and nothing
# for any other path. A path that every module reads selects all of them.
direct_module() {
    local path="$1"
    case "$path" in
    tests/it/main.rs | tests/it/support/* | tests/fixtures/* | assets/*)
        echo "it:ALL"
        ;;
    tests/it/*)
        module_label it "${path#tests/it/}"
        ;;
    esac
    return 0
}

# Prints "<kind>:<module>" for $2, a path relative to the module directory: the
# first directory name, or the file name without .rs. A file that is not a .rs
# module, such as a README in the directory, selects every module of the kind.
module_label() {
    local kind="$1" rest="$2" name
    case "$rest" in
    */*) name="${rest%%/*}" ;;
    *.rs) name="${rest%.rs}" ;;
    *)
        echo "$kind:ALL"
        return 0
        ;;
    esac
    echo "$kind:$name"
}

# Prints the integration test modules to run for the changed paths, one per
# line, for --covering. A module is one whose covers line names the package a
# path belongs to; a path under a module directory selects that module alone.
covering_modules() {
    local table path direct
    table="$(covers_table)"
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        direct="$(direct_module "$path")"
        if [ -n "$direct" ]; then
            printf '%s\n' "$direct"
            continue
        fi
        classify_path "$path"
        case "$CLASSIFIED" in
        "" | ALL) continue ;;
        *) PACKAGE="$CLASSIFIED" ;;
        esac
        awk -v package="$PACKAGE" '$2 == package { print $1 }' <<<"$table"
    done | sort -u
}

if [ "$mode" = "--touched" ]; then
    touched_packages
    exit 0
fi
if [ "$mode" = "--covering" ]; then
    covering_modules
    exit 0
fi

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

if [ -n "$all_path" ]; then
    echo "--workspace"
    exit 0
fi

out=""
while IFS= read -r line; do
    [ -n "$line" ] || continue
    pkg="${line%%:*}"
    if has "$pkg"; then
        out="${out:+$out }-p $pkg"
    fi
done <<<"$graph"
printf '%s\n' "$out"
