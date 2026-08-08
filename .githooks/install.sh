#!/usr/bin/env bash
# Install git hooks for residuum
# Run this once after cloning the repository

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
# Inside a linked worktree, .git is a file (not a directory) pointing at the
# worktree's private git dir, and hooks live in the common dir shared by all
# worktrees — not under a nonexistent "$REPO_ROOT/.git/hooks".
GIT_COMMON_DIR="$(git -C "$REPO_ROOT" rev-parse --path-format=absolute --git-common-dir)"
HOOK_DIR="$GIT_COMMON_DIR/hooks"

echo "Symlinking .githooks into .git/hooks..."

for hook in "$SCRIPT_DIR"/pre-commit "$SCRIPT_DIR"/commit-msg; do
    name="$(basename "$hook")"
    ln -sf "$hook" "$HOOK_DIR/$name"
    echo "  $name -> .githooks/$name"
done

# Clear core.hooksPath if set — symlinks make it unnecessary
git -C "$REPO_ROOT" config --unset core.hooksPath 2>/dev/null || true

echo ""
echo "Git hooks installed successfully!"
