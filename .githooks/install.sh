#!/usr/bin/env bash
#
# Install the tracked hooks into the hooks directory shared by every worktree of
# this repo.
#
# Each hook there is a small dispatcher shim, not a symlink. The hooks directory
# is shared, so a static link can name only one checkout: a commit made in
# another worktree would run this checkout's hook. The shim runs the hook from
# the worktree git is working in, and falls back to the main checkout's copy
# when that worktree's branch predates the hook. The tracked hooks stay the
# single source, so an edit to one still takes effect immediately and stays
# under version control.
#
# Re-run this after changing the install. It replaces whatever is in the hooks
# directory, including symlinks that earlier installs left behind.
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

for hook in pre-commit commit-msg post-commit pre-push; do
    chmod +x "$SCRIPT_DIR/$hook"
    # Write beside the target, then rename over it. rename(2) replaces a symlink
    # entry rather than writing through it, so an old symlink to the tracked hook
    # is left alone and the swap is atomic.
    tmp="$(mktemp "$HOOK_DIR/.$hook.XXXXXX")"
    cat >"$tmp" <<'SHIM'
#!/usr/bin/env bash
#
# Dispatcher for a git hook, written by .githooks/install.sh. Git runs hooks from
# the shared hooks directory for every worktree, so this runs the tracked hook
# from the worktree git is working in. The hook's stdin and exit status pass
# straight through.
set -euo pipefail

hook="${0##*/}"
target="$(git rev-parse --show-toplevel)/.githooks/$hook"
if [ ! -f "$target" ]; then
    # A worktree on a branch that predates this hook has no copy of it.
    main="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"
    target="$main/.githooks/$hook"
fi
if [ ! -f "$target" ]; then
    echo "$hook: no tracked hook in this worktree or the main checkout. Delete the stale hook at $0, or re-run .githooks/install.sh from the main checkout." >&2
    exit 1
fi
exec "$target" "$@"
SHIM
    # mktemp creates the file 0600, and bash must be able to read a script to run it.
    chmod 755 "$tmp"
    mv -f "$tmp" "$HOOK_DIR/$hook"
    echo "installed $hook"
done

# core.hooksPath, if set, wins over .git/hooks and would bypass the shims above.
# Clear it so there is exactly one place hooks come from.
git -C "$REPO_ROOT" config --unset core.hooksPath 2>/dev/null || true

echo "Hooks installed. Bypassing them with --no-verify is forbidden."
