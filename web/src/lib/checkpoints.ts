// ── Shared formatting for the checkpoint history UI ───────────────────
//
// Used by the Settings → History view and a file's history in Files and
// Shared files. See `docs/systems-usage/checkpoints.md`.

import type { ChangeKind, CheckpointTrigger, RepoKind, UndoOutcome } from "./types";

/** The repositories a History shows, in order: an agent's two, or the team's and the hub's for `null`. */
export function historyRepos(agent: string | null): readonly RepoKind[] {
  return agent === null ? ["team", "hub"] : ["workspace", "agent_config"];
}

/** What each repository holds, as History names it. */
export const REPO_LABELS: Readonly<Record<RepoKind, string>> = {
  workspace: "Workspace",
  agent_config: "Config files",
  team: "Shared files",
  hub: "Install-wide config",
};

export const CHANGE_LABELS: Readonly<Record<ChangeKind, string>> = {
  added: "Added",
  modified: "Changed",
  deleted: "Removed",
};

/** What undoing a checkpoint did, in a sentence or two. */
export function undoReport({
  reverted_paths: reverted,
  skipped_paths: skipped,
}: UndoOutcome): string {
  const parts: string[] = [];
  if (reverted.length > 0) parts.push(`Put back ${reverted.join(", ")}.`);
  if (skipped.length > 0) {
    const them = skipped.length === 1 ? "it changed" : "they changed";
    parts.push(`Left ${skipped.join(", ")} alone, because ${them} again since.`);
  }
  return parts.length > 0 ? parts.join(" ") : "Nothing needed undoing.";
}

/** How one line of a unified diff reads: added, removed, a header, or unchanged context. */
export function diffLineKind(line: string): "added" | "removed" | "meta" | "context" {
  if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("@@")) return "meta";
  if (line.startsWith("+")) return "added";
  if (line.startsWith("-")) return "removed";
  return "context";
}

/** Human label for a checkpoint's trigger, as shown in the history list. */
export function triggerLabel(trigger: CheckpointTrigger): string {
  const labels: Record<CheckpointTrigger, string> = {
    turn_start: "Turn start",
    turn_end: "Turn end",
    pre_action: "Before action",
    pre_config_write: "Before save",
    restore: "Restore",
    undo: "Undo",
  };
  return labels[trigger];
}

/** `1.2 KB` / `3.4 MB` style size, matching the app's other byte formatters. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * The config repo's encrypted key stores: checkpoints track them like any
 * other file, but their content is ciphertext, so the UI must never try to
 * show a diff or preview it — only that it changed, with a restore action.
 */
const ENCRYPTED_CONFIG_FILES = new Set(["secrets.toml.enc", "agent-keys.toml.enc"]);

export function isEncryptedConfigFile(path: string): boolean {
  return ENCRYPTED_CONFIG_FILES.has(path);
}

/** What restoring an encrypted store says it does, shown in place of a diff. */
export function encryptedRestoreHint(path: string): string {
  if (path === "agent-keys.toml.enc") {
    return "Restores the saved agent keys to how they were at this point.";
  }
  return "Restores the saved secrets to how they were at this point.";
}
