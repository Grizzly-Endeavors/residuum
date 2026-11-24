import type { WorkspaceScope } from "./hub-types";
import type { WorkspaceChange } from "./types";

export function treeWatchPrefix(scope: WorkspaceScope): string {
  return scope === "team" ? "team" : "";
}

function treeRelativePath(path: string, scope: WorkspaceScope): string | null {
  if (scope !== "team") return path === "" ? null : path;
  return path.startsWith("team/") ? path.slice("team/".length) : null;
}

function parentOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

function isWithin(dir: string, ancestor: string): boolean {
  return dir === ancestor || dir.startsWith(`${ancestor}/`);
}

export interface TreeUpdate {
  reload: string[];
  forget: string[];
}

export function treeUpdateFor(
  changes: readonly WorkspaceChange[],
  scope: WorkspaceScope,
  listed: readonly string[],
): TreeUpdate {
  const removed: string[] = [];
  const touched = new Set<string>();
  for (const change of changes) {
    if (change.kind === "modified") continue;
    const path = treeRelativePath(change.path, scope);
    if (path === null) continue;
    if (change.kind === "removed") removed.push(path);
    touched.add(parentOf(path));
  }
  const gone = (dir: string): boolean => removed.some((path) => isWithin(dir, path));
  return {
    reload: listed.filter((dir) => touched.has(dir) && !gone(dir)),
    forget: listed.filter(gone),
  };
}
