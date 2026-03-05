#!/usr/bin/env bash
# Install git hooks for residuum
# Run this once after cloning the repository

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
HOOK_DIR="$REPO_ROOT/.git/hooks"

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
