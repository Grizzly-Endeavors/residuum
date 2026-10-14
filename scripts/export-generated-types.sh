#!/usr/bin/env bash
# Exports the TypeScript protocol types that ts-rs generates from the Rust types
# into the directory given as the argument, which must be an absolute path. The
# checkout is not written. The check and the regeneration scripts both use this.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <absolute-output-dir>" >&2
    exit 2
fi

# cargo runs git subprocesses (cargo-deny's advisory-db refresh among them) that
# must not inherit the repository variables a hook exports. See .githooks/pre-commit.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR

RESIDUUM_TS_EXPORT_DIR="$1" cargo test --quiet --test ts_export

# A test filter that matches nothing still exits 0, so check the export wrote its output.
if [ ! -f "$1/ClientMessage.ts" ]; then
    echo "error: the ts-rs export wrote no files to $1" >&2
    exit 1
fi
