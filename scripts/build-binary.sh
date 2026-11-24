#!/usr/bin/env bash
set -euo pipefail

profile="${1:-}"
case "$profile" in
    debug) cargo_args=() ;;
    release) cargo_args=(--release) ;;
    *)
        echo "usage: scripts/build-binary.sh debug|release" >&2
        exit 2
        ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$root"

exe=""
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) exe=".exe" ;;
esac

cargo build ${cargo_args[@]+"${cargo_args[@]}"}

built="${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-target}}/$profile/residuum$exe"
dest_dir="target/$profile"
dest="$dest_dir/residuum$exe"
if [ ! -f "$built" ]; then
    echo "error: cargo build finished but $built is missing, so there is no binary to copy to $dest" >&2
    exit 1
fi
mkdir -p "$dest_dir"
if [ ! "$built" -ef "$dest" ]; then
    # Copy beside the destination, then rename over it: a running binary can't
    # be opened for writing on Linux, but it can be replaced by a rename.
    tmp="$dest_dir/.residuum$exe.$$"
    trap 'rm -f "$tmp"' EXIT
    cp -- "$built" "$tmp"
    mv -f -- "$tmp" "$dest"
    trap - EXIT
fi
echo "Built $dest (cargo built it in ${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-target}})." >&2
