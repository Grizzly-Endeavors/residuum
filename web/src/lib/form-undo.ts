// ── Undo for a removal from a Settings form array (Providers, MCP servers,
//    Integrations webhooks) ──────────────────────────────────────────
//
// These arrays only exist in local form state until the debounced autosave
// (see `Settings.svelte`) turns their diff into a PATCH — so removing an
// entry doesn't, by itself, tell you whether it's actually gone from the
// file on disk yet. Undoing it needs to know which of two things happened:
//
// - The save hasn't written the removal: nothing was sent, so putting the
//   entry back in the array (and cancelling a pending save, so it doesn't
//   save a form state the array itself has un-done) is completely safe.
// - A save already wrote it: the PATCH deletes the whole entry, including
//   any field the form doesn't parse back out (an MCP server's
//   `type`/`headers` on an unrelated transport, a provider's unmodeled
//   field, ...). Rebuilding it from form state would silently drop those.
//   The only lossless undo is restoring the file from the checkpoint the
//   server took just before that write — the same pattern `notifyWithUndo`
//   (lib/undo.ts) uses for an immediate delete endpoint, resolved after the
//   fact because the write may not have happened when Undo is clicked.

import { undoLastAction } from "./api";
import { userErrorMessage } from "./errors";
import type { PendingSaveTracker } from "./pending-save";
import { toast } from "./toast.svelte";
import type { RepoKind } from "./types";

/**
 * Show a success toast for removing an entry from an autosaving form
 * array/field, with an Undo that's safe either way the save landed.
 *
 * Call it right after the removal. `revertLocally` puts the entry back in
 * the form's local state (called only when no save has written the
 * removal). `repo`/`path` name the file this array's changes are saved to,
 * as recorded with `PendingSaveTracker.recordWrite`, for the checkpoint
 * restore when one did.
 */
export function notifyFormUndo(
  message: string,
  pendingSave: PendingSaveTracker,
  revertLocally: () => void,
  repo: RepoKind,
  path: string,
  onRestored?: () => void | Promise<void>,
): void {
  const removedAt = pendingSave.mark();
  toast.success(message, {
    label: "Undo",
    onClick: () => {
      if (pendingSave.cancelIfPending()) {
        revertLocally();
        return;
      }
      void undoAfterSettle(pendingSave, removedAt, revertLocally, repo, path, onRestored);
    },
  });
}

async function undoAfterSettle(
  pendingSave: PendingSaveTracker,
  removedAt: number,
  revertLocally: () => void,
  repo: RepoKind,
  path: string,
  onRestored?: () => void | Promise<void>,
): Promise<void> {
  // A save already in flight may be the one writing the removal; its
  // checkpoint id is only known once it settles.
  await pendingSave.waitForSettled();
  const write = pendingSave.firstWriteAfter(removedAt, path);
  if (write.kind === "not-written") {
    // The save failed or never ran, so the file still has the entry.
    revertLocally();
    return;
  }
  if (!write.checkpointId) {
    toast.error(
      `Couldn't undo — no checkpoint of ${path} was saved before that change. Add the entry back by hand.`,
    );
    return;
  }
  try {
    await undoLastAction(write.checkpointId, repo, path);
    toast.success("Restored.");
    await onRestored?.();
  } catch (err: unknown) {
    toast.error(userErrorMessage(err, { action: "Couldn't undo that." }));
  }
}
