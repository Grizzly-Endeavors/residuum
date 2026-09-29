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
 * Show a success toast for a destructive action that just completed.
 * When `checkpointId` is the id the action returned, the toast carries
 * Undo for that checkpoint. When it is null — the checkpoint failed, so
 * there is nothing correct to restore — the toast has no Undo.
 *
 * `path` may list several paths (an artifact's page and its data files); Undo
 * restores each from the same checkpoint.
 *
 * `onRestored` runs after a successful undo, so the caller can refresh
 * whatever list or view showed the now-gone item.
 */
export function notifyWithUndo(
  message: string,
  repo: RepoKind,
  path: string | string[],
  checkpointId: string | null,
  onRestored?: () => void | Promise<void>,
): void {
  if (!checkpointId) {
    toast.success(message);
    return;
  }
  toast.success(message, {
    label: "Undo",
    onClick: () => {
      void runUndo(checkpointId, repo, path, onRestored);
    },
  });
}

async function runUndo(
  checkpointId: string,
  repo: RepoKind,
  path: string | string[],
  onRestored?: () => void | Promise<void>,
): Promise<void> {
  try {
    for (const each of Array.isArray(path) ? path : [path]) {
      await undoLastAction(checkpointId, repo, each);
    }
    toast.success("Restored.");
    await onRestored?.();
  } catch (err) {
    toast.error(userErrorMessage(err, { action: "Couldn't undo that." }));
  }
}
