// ── Shared formatting for the checkpoint history UI ───────────────────
//
// Used by the Settings → History view and the per-file history panel
// opened from the Workspace file browser. See
// `docs/systems-usage/checkpoints.md`.

import type { CheckpointTrigger } from "./types";

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
