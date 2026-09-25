// ── Single-click destructive actions, backed by a checkpoint undo ────
//
// Residuum's confirm-before-act pattern (a two-click "Remove?" button) is
// gone: a destructive action fires on the first click, and its result toast
// carries a direct "Undo" instead of asking the user to confirm ahead of
// time. This works because the server checkpoints the affected repository
// immediately before every one of these actions runs — see
// `docs/systems-usage/checkpoints.md` — so undoing is always just restoring
// that pre-action checkpoint.

import { undoLastAction } from "./api";
import { userErrorMessage } from "./errors";
import { toast } from "./toast.svelte";
import type { RepoKind } from "./types";

/**
 * Show a success toast for a destructive action that just completed, with
 * a direct "Undo" that restores `path` from the checkpoint taken just
 * before it. Call this after the action's own API call resolves.
 *
 * `onRestored` runs after a successful undo, so the caller can refresh
 * whatever list or view showed the now-gone item.
 */
export function notifyWithUndo(
  message: string,
  repo: RepoKind,
  path: string,
  onRestored?: () => void | Promise<void>,
): void {
  toast.success(message, {
    label: "Undo",
    onClick: () => {
      void runUndo(repo, path, onRestored);
    },
  });
}

async function runUndo(
  repo: RepoKind,
  path: string,
  onRestored?: () => void | Promise<void>,
): Promise<void> {
  try {
    const outcome = await undoLastAction(repo, path);
    if (!outcome) {
      toast.error("Nothing to restore — no checkpoint was found for this.");
      return;
    }
    toast.success("Restored.");
    await onRestored?.();
  } catch (err) {
    toast.error(userErrorMessage(err, { action: "Couldn't undo that." }));
  }
}
