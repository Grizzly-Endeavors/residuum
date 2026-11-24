import type { ChangeKind, CheckpointTrigger, RepoKind, UndoOutcome } from "./types";

export function historyRepos(agent: string | null): readonly RepoKind[] {
  return agent === null ? ["team", "hub"] : ["workspace", "agent_config"];
}

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

export function diffLineKind(line: string): "added" | "removed" | "meta" | "context" {
  if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("@@")) return "meta";
  if (line.startsWith("+")) return "added";
  if (line.startsWith("-")) return "removed";
  return "context";
}

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

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const ENCRYPTED_CONFIG_FILES = new Set(["secrets.toml.enc", "agent-keys.toml.enc"]);

export function isEncryptedConfigFile(path: string): boolean {
  return ENCRYPTED_CONFIG_FILES.has(path);
}

export function encryptedRestoreHint(path: string): string {
  if (path === "agent-keys.toml.enc") {
    return "Restores the saved agent keys to how they were at this point.";
  }
  return "Restores the saved secrets to how they were at this point.";
}
