#!/usr/bin/env bash
# Install git hooks for ironclaw
# Run this once after cloning the repository

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

echo "Configuring git to use .githooks directory..."
git -C "$REPO_ROOT" config core.hooksPath .githooks

echo "Git hooks installed successfully!"
echo ""
echo "Hooks enabled:"
echo "  - pre-commit: formatting, clippy, tests"
echo "  - commit-msg: message validation"
echo "  - pre-push: full test suite"
echo ""
echo "To bypass hooks temporarily (not recommended):"
echo "  git commit --no-verify"
echo "  git push --no-verify"
