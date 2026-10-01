// Which tree a file lives in, and what that means for it. Files shows an
// agent's workspace, where `team/…` reaches the team's files; Shared files and
// its `panel=file:<path>` show the team's folder, with paths relative to it.

import { agentConfigFile, type ConfigFile } from "../../lib/config-coordinator";
import type { WorkspaceScope } from "../../lib/hub-types";
import { isAgentPlace, type Place } from "../../lib/routes";
import type { RepoKind } from "../../lib/types";
import { checkpointLocation } from "../../lib/undo";

export interface FileSource {
  /** The agent whose workspace holds the path; null for the team's folder. */
  agent: string | null;
  scope: WorkspaceScope;
}

/** Where a file panel's path is read on `place`, or null on a place that can't show one. */
export function fileSourceFor(place: Place): FileSource | null {
  if (isAgentPlace(place)) return { agent: place.agent, scope: "agent" };
  if (place.kind === "shared-files") return { agent: null, scope: "team" };
  return null;
}

export function sameSource(a: FileSource | null, b: FileSource | null): boolean {
  return a !== null && b !== null && a.agent === b.agent && a.scope === b.scope;
}

/** The last segment of a path, for a title. */
export function fileName(path: string): string {
  const segments = path.split("/").filter((segment) => segment !== "");
  return segments.at(-1) ?? path;
}

/** The folder a path is in, `""` at the root. */
export function parentDir(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

/** The path in an agent's namespace, where the team's files sit under `team/`. */
function workspacePath(source: FileSource, path: string): string {
  return source.scope === "team" ? `team/${path}` : path;
}

/** The agent's own identity files, and the team's rules and user facts. */
const IDENTITY_FILES: ReadonlySet<string> = new Set([
  "SOUL.md",
  "HEARTBEAT.yml",
  "team/AGENTS.md",
  "team/USER.md",
]);

/** Whether the tree tints `path` as an identity file. */
export function isIdentityFile(source: FileSource, path: string): boolean {
  return IDENTITY_FILES.has(workspacePath(source, path));
}

const CONFIG_FILES = {
  "config/config.toml": "config",
  "config/providers.toml": "providers",
  "config/mcp.json": "mcp",
} as const;

/**
 * The config file `path` is, or null. These are written only through the
 * config write coordinator, so Settings and every view showing their values
 * hear about the change.
 */
export function configFileAt(source: FileSource, path: string): ConfigFile | null {
  if (source.scope !== "agent" || source.agent === null || !Object.hasOwn(CONFIG_FILES, path))
    return null;
  return agentConfigFile(source.agent, CONFIG_FILES[path as keyof typeof CONFIG_FILES]);
}

/**
 * Where a file's history is kept. An agent's `config.toml` and
 * `providers.toml` are in its agent-config repository (they can hold keys),
 * team files in the team repository, the rest in the workspace repository.
 */
export function historyLocation(
  source: FileSource,
  path: string,
): { repo: RepoKind; path: string } {
  if (source.scope === "team") return { repo: "team", path };
  if (path === "config/config.toml" || path === "config/providers.toml") {
    return { repo: "agent_config", path: fileName(path) };
  }
  return checkpointLocation(path);
}
