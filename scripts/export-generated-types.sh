#!/usr/bin/env bash
# Exports the TypeScript protocol types that ts-rs generates from the Rust types
# into the directory given as the argument, which must be an absolute path. The
# checkout is not written. The check and the regeneration scripts both use this.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <absolute-output-dir>" >&2
    exit 2
fi
case "$1" in
    /*) ;;
    *)
        echo "error: the output dir must be an absolute path, got $1" >&2
        exit 2
        ;;
esac

# cargo runs git subprocesses (cargo-deny's advisory-db refresh among them) that
# must not inherit the repository variables a hook exports. See .githooks/pre-commit.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_COMMON_DIR

RESIDUUM_TS_EXPORT_DIR="$1" cargo test --quiet -p residuum --test ts_export

# cargo test exits 0 when no test matches, as after the export test is renamed or
# removed, so check that the export wrote its output.
if [ ! -f "$1/ClientMessage.ts" ]; then
    echo "error: the ts-rs export wrote no files to $1" >&2
    exit 1
fi
