// ── Single-click destructive actions, backed by a checkpoint undo ────
//
// Residuum's confirm-before-act pattern (a two-click "Remove?" button) is
// gone: a destructive action fires on the first click, and its result toast
// carries a direct "Undo" instead of asking the user to confirm ahead of
// time. This works because the server checkpoints the affected repository
// immediately before every one of these actions runs — see
// `docs/systems-usage/checkpoints.md` — so undoing is always just restoring
// that pre-action checkpoint.

import { undoLastAction, type WorkspaceCheckpoint } from "./api";
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

/** One restore an Undo performs: `path` is relative to `repo`'s root. */
export interface RestoreTarget {
  checkpointId: string;
  repo: RepoKind;
  path: string;
}

const TEAM_PREFIX = "team/";

/**
 * Locate a workspace-API tree path in its checkpoint repository: `team/...`
 * lives in the team repository relative to `team/`; anything else lives in
 * the workspace repository under its own path.
 */
export function checkpointLocation(path: string): { repo: RepoKind; path: string } {
  if (path.startsWith(TEAM_PREFIX)) {
    return { repo: "team", path: path.slice(TEAM_PREFIX.length) };
  }
  return { repo: "workspace", path };
}

/**
 * Pick the restores that undo an action on workspace-API `paths`. A
 * `team/...` path lives in the team repository, relative to `team/`; any
 * other path lives in the workspace repository. Each path restores from the
 * checkpoint of its own repository, and a path whose repository recorded no
 * checkpoint is left out (nothing correct to restore it from).
 */
export function restoreTargets(
  paths: string[],
  checkpoints: WorkspaceCheckpoint[],
): RestoreTarget[] {
  const targets: RestoreTarget[] = [];
  for (const path of paths) {
    const location = checkpointLocation(path);
    const checkpoint = checkpoints.find((c) => c.repo === location.repo);
    if (!checkpoint) continue;
    targets.push({ checkpointId: checkpoint.id, ...location });
  }
  return targets;
}

/**
 * `notifyWithUndo` for the workspace file API, whose paths may be agent
 * files or `team/...` files and whose response names one checkpoint per
 * repository. Undo restores each path from the right repository.
 */
export function notifyWithWorkspaceUndo(
  message: string,
  paths: string | string[],
  checkpoints: WorkspaceCheckpoint[],
  onRestored?: () => void | Promise<void>,
): void {
  const targets = restoreTargets(Array.isArray(paths) ? paths : [paths], checkpoints);
  if (targets.length === 0) {
    toast.success(message);
    return;
  }
  toast.success(message, {
    label: "Undo",
    onClick: () => {
      void runRestoreTargets(targets, onRestored);
    },
  });
}

async function runRestoreTargets(
  targets: RestoreTarget[],
  onRestored?: () => void | Promise<void>,
): Promise<void> {
  try {
    for (const target of targets) {
      await undoLastAction(target.checkpointId, target.repo, target.path);
    }
    toast.success("Restored.");
    await onRestored?.();
  } catch (err) {
    toast.error(userErrorMessage(err, { action: "Couldn't undo that." }));
  }
}
