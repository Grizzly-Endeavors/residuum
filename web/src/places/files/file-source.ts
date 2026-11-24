import { agentConfigFile, type ConfigFile } from "../../lib/config-coordinator";
import type { WorkspaceScope } from "../../lib/hub-types";
import { isAgentPlace, type Panel, type Place } from "../../lib/routes";
import type { RepoKind } from "../../lib/types";
import { checkpointLocation } from "../../lib/undo";

export interface FileSource {
  agent: string | null;
  scope: WorkspaceScope;
}

type FilePanelSpec = Extract<Panel, { kind: "file" }>;

export function fileSourceFor(place: Place): FileSource | null {
  if (isAgentPlace(place)) return { agent: place.agent, scope: "agent" };
  if (place.kind === "shared-files") return { agent: null, scope: "team" };
  return null;
}

export function panelFileSource(place: Place, panel: FilePanelSpec): FileSource | null {
  if (panel.agent !== null) return { agent: panel.agent, scope: "agent" };
  return fileSourceFor(place);
}

export function sameSource(a: FileSource | null, b: FileSource | null): boolean {
  return a !== null && b !== null && a.agent === b.agent && a.scope === b.scope;
}

export function fileName(path: string): string {
  const segments = path.split("/").filter((segment) => segment !== "");
  return segments.at(-1) ?? path;
}

export function parentDir(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

function workspacePath(source: FileSource, path: string): string {
  return source.scope === "team" ? `team/${path}` : path;
}

const IDENTITY_FILES: ReadonlySet<string> = new Set([
  "SOUL.md",
  "HEARTBEAT.yml",
  "team/AGENTS.md",
  "team/USER.md",
]);

export function isIdentityFile(source: FileSource, path: string): boolean {
  return IDENTITY_FILES.has(workspacePath(source, path));
}

const CONFIG_FILES = {
  "config/config.toml": "config",
  "config/providers.toml": "providers",
  "config/mcp.json": "mcp",
} as const;

export function configFileAt(source: FileSource, path: string): ConfigFile | null {
  if (source.scope !== "agent" || source.agent === null || !Object.hasOwn(CONFIG_FILES, path))
    return null;
  return agentConfigFile(source.agent, CONFIG_FILES[path as keyof typeof CONFIG_FILES]);
}

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
