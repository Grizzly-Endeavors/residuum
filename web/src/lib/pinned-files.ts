/**
 * Files the workspace tree marks as identity files: the agent's own files at
 * the agent root, and the team's rules and user facts under the `team/`
 * folder.
 */
const PINNED_FILE_PATHS: ReadonlySet<string> = new Set([
  "SOUL.md",
  "HEARTBEAT.yml",
  "CHANNELS.yml",
  "team/AGENTS.md",
  "team/USER.md",
]);

/** Whether the file at workspace path `path` is an identity file. */
export function isPinnedFile(path: string): boolean {
  return PINNED_FILE_PATHS.has(path);
}
