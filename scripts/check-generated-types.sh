#!/usr/bin/env bash
# Fails when the TypeScript protocol types in web/src/lib/generated differ from
# what the Rust types generate. CI runs this, pre-push runs it, and `just
# types-check` runs it locally. Fix a failure with `just types`, then commit the
# result.
#
# The comparison is the fresh export against the index, so stage regenerated
# files before running it locally. The export goes to a scratch directory and
# the index is read with git's plumbing, so the check never writes to the
# checkout's files or its index. pre-push runs it in place on a clean checkout.
set -euo pipefail

dir=web/src/lib/generated
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# cargo runs git subprocesses (cargo-deny's advisory-db refresh among them) that
# must not inherit the repository variables a hook exports. See .githooks/pre-commit.
(
    unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR
    RESIDUUM_TS_EXPORT_DIR="$work/generated" cargo test --quiet --test ts_export
)

# The index's copies of the directory. checkout-index with a prefix writes them
# out without touching the index or the working tree.
mkdir -p "$work/index/$dir"
git ls-files -z -- "$dir" | git checkout-index --prefix="$work/index/" -z --stdin

# protocol.ts is the hand-written barrel and is not exported, so it is left out
# of both sides.
if ! diff -r -x protocol.ts "$work/index/$dir" "$work/generated"; then
    echo "error: $dir is out of date with the Rust types. Run 'just types' and commit the result." >&2
    exit 1
fi
