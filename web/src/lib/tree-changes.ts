// ── File tree and the change feed ────────────────────────────────────
//
// A file tree that stays current watches its whole tree and, on each batch of
// changes, works out which of the folders it has listed now show something
// different. This is that arithmetic, separate from the component that
// fetches and draws.

import type { WorkspaceScope } from "./hub-types";
import type { WorkspaceChange } from "./types";

/** The prefix that covers a whole tree: the team tree is the `team/` folder, an agent's is everything. */
export function treeWatchPrefix(scope: WorkspaceScope): string {
  return scope === "team" ? "team" : "";
}

/** A change's path relative to the tree's root, or `null` for one outside the tree or at its root. */
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

/** What a batch of changes means for the folders a tree has listed. */
export interface TreeUpdate {
  /** Folders whose listing changed and that still exist: list them again. */
  reload: string[];
  /** Folders that no longer exist: forget their listings. */
  forget: string[];
}

/**
 * What `changes` do to a tree that has listed the folders `listed` (paths
 * relative to the tree's root, `""` for the root itself). The tree shows
 * names, so only a path appearing or disappearing changes a listing: a file
 * whose content changed looks the same. A removed folder takes the folders
 * listed under it with it.
 */
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
