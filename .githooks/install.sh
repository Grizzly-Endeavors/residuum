#!/usr/bin/env bash
#
# Symlink the tracked hooks into this repo's hooks directory.
#
# Symlinks rather than copies so an edit to a hook takes effect immediately and
# stays under version control — a copied hook silently drifts from the one in
# the repo, which is the worst of both worlds.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

# Ask git for the hooks directory rather than assuming `$REPO_ROOT/.git` is one.
# Inside a linked worktree `.git` is a *file* pointing at the real gitdir, so the
# assumption fails outright — and hooks live in the common dir shared by every
# worktree regardless.
GIT_COMMON_DIR="$(git -C "$REPO_ROOT" rev-parse --path-format=absolute --git-common-dir)"
HOOK_DIR="$GIT_COMMON_DIR/hooks"
mkdir -p "$HOOK_DIR"

for hook in pre-commit commit-msg post-commit; do
    chmod +x "$SCRIPT_DIR/$hook"
    ln -sf "$SCRIPT_DIR/$hook" "$HOOK_DIR/$hook"
    echo "installed $hook"
done

# core.hooksPath, if set, wins over .git/hooks and would make the symlinks above
# dead weight. Clear it so there is exactly one place hooks come from.
git -C "$REPO_ROOT" config --unset core.hooksPath 2>/dev/null || true

echo "Hooks installed. Bypassing them with --no-verify is forbidden."
