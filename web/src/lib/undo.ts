import type { WorkspaceCheckpoint } from "./api";
import { configCoordinator } from "./config-coordinator";
import { userErrorMessage } from "./errors";
import { toast } from "./toast.svelte";
import type { RepoKind } from "./types";

export function notifyWithUndo(
  agent: string | null,
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
      void runUndo(agent, checkpointId, repo, path, onRestored);
    },
  });
}

async function runUndo(
  agent: string | null,
  checkpointId: string,
  repo: RepoKind,
  path: string | string[],
  onRestored?: () => void | Promise<void>,
): Promise<void> {
  try {
    for (const each of Array.isArray(path) ? path : [path]) {
      await configCoordinator.restore(agent, checkpointId, repo, each);
    }
    toast.success("Restored.");
    await onRestored?.();
  } catch (err) {
    toast.error(userErrorMessage(err, { action: "Couldn't undo that." }));
  }
}

export interface RestoreTarget {
  checkpointId: string;
  repo: RepoKind;
  path: string;
}

const TEAM_PREFIX = "team/";

export function checkpointLocation(path: string): { repo: RepoKind; path: string } {
  if (path.startsWith(TEAM_PREFIX)) {
    return { repo: "team", path: path.slice(TEAM_PREFIX.length) };
  }
  return { repo: "workspace", path };
}

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

export function notifyWithWorkspaceUndo(
  agent: string | null,
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
      void runRestoreTargets(agent, targets, onRestored);
    },
  });
}

async function runRestoreTargets(
  agent: string | null,
  targets: RestoreTarget[],
  onRestored?: () => void | Promise<void>,
): Promise<void> {
  try {
    for (const target of targets) {
      await configCoordinator.restore(agent, target.checkpointId, target.repo, target.path);
    }
    toast.success("Restored.");
    await onRestored?.();
  } catch (err) {
    toast.error(userErrorMessage(err, { action: "Couldn't undo that." }));
  }
}
