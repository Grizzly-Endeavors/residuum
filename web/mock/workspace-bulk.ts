import type { WorkspaceEntry } from "../src/lib/types";
import type { MockState } from "./state";
import { byName } from "./util";
import { byteLength, entryOf, listDirectory, pathKind } from "./workspace-tree";

/**
 * The bulk reads of the workspace API: a recursive listing and a batch read,
 * with the backend's budgets. A file's content over `FILE_LIMIT_BYTES`, or past
 * the response's `RESPONSE_BUDGET_BYTES`, is left out and the entry says why.
 */
const FILE_LIMIT_BYTES = 1024 * 1024;
const RESPONSE_BUDGET_BYTES = 8 * 1024 * 1024;

export interface TreeEntry {
  path: string;
  type: WorkspaceEntry["entry_type"];
  size?: number;
  modified: number;
  version: string;
  content?: string;
  skipped?: "too_large" | "budget";
}

export interface TreeResponse {
  path: string;
  entries: TreeEntry[];
  listing_truncated: boolean;
  content_truncated: boolean;
}

export interface BatchFile {
  path: string;
  size?: number;
  modified?: number;
  version?: string;
  content?: string;
  error?: "not_found" | "blocked" | "is_directory" | "too_large" | "budget";
}

export interface BatchReadResponse {
  files: BatchFile[];
  content_truncated: boolean;
}

export interface Place {
  state: MockState;
  key: string;
}

export interface Walker {
  stateAt: (key: string) => MockState;
  label: (key: string) => string;
}

export interface TreeOptions {
  content: boolean;
  globs: readonly string[];
  depth: number | null;
}

function globPattern(glob: string): RegExp {
  const source = glob
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*\*\/?/g, "\0")
    .replace(/\*/g, "[^/]*")
    .replace(/\?/g, "[^/]")
    .replace(/\0/g, ".*");
  return new RegExp(`^${source}$`);
}

function matchesGlobs(globs: readonly string[], relative: string): boolean {
  const name = relative.slice(relative.lastIndexOf("/") + 1);
  return globs.some((glob) => globPattern(glob).test(glob.includes("/") ? relative : name));
}

export function buildTree(root: string, walker: Walker, options: TreeOptions): TreeResponse {
  const found: Array<{ key: string; entry: TreeEntry }> = [];
  const visit = (dirKey: string, relativeDir: string, level: number): void => {
    for (const listed of listDirectory(walker.stateAt(dirKey), dirKey) ?? []) {
      const key = dirKey === "" ? listed.name : `${dirKey}/${listed.name}`;
      const relative = relativeDir === "" ? listed.name : `${relativeDir}/${listed.name}`;
      const entry: TreeEntry = {
        path: walker.label(key),
        type: listed.entry_type,
        modified: listed.modified,
        version: listed.version,
      };
      if (listed.entry_type === "directory") {
        if (options.globs.length === 0) found.push({ key, entry });
        if (options.depth === null || level < options.depth) visit(key, relative, level + 1);
      } else if (options.globs.length === 0 || matchesGlobs(options.globs, relative)) {
        found.push({ key, entry: { ...entry, size: listed.size ?? 0 } });
      }
    }
  };
  if (options.depth === null || options.depth >= 1) visit(root, "", 1);
  found.sort((a, b) => byName(a.entry.path, b.entry.path));

  let contentTruncated = false;
  let used = 0;
  for (const { key, entry } of options.content ? found : []) {
    const text = entry.type === "file" ? walker.stateAt(key).workspaceFileContents[key] : undefined;
    if (text === undefined) continue;
    if (byteLength(text) > FILE_LIMIT_BYTES) {
      entry.skipped = "too_large";
    } else if (used + byteLength(text) > RESPONSE_BUDGET_BYTES) {
      entry.skipped = "budget";
      contentTruncated = true;
    } else {
      entry.content = text;
      used += byteLength(text);
    }
  }
  return {
    path: walker.label(root),
    entries: found.map((f) => f.entry),
    listing_truncated: false,
    content_truncated: contentTruncated,
  };
}

export function batchRead(
  paths: readonly string[],
  place: (path: string) => Place | null,
): BatchReadResponse {
  let contentTruncated = false;
  let used = 0;
  const files = paths.map((path): BatchFile => {
    const at = place(path);
    if (at === null) return { path, error: "blocked" };
    const kind = pathKind(at.state, at.key);
    if (kind === null) return { path, error: "not_found" };
    if (kind === "directory") return { path, error: "is_directory" };
    const content = at.state.workspaceFileContents[at.key] ?? "";
    const entry = entryOf(at.state, at.key);
    const meta = {
      path,
      size: byteLength(content),
      modified: entry?.modified,
      version: entry?.version,
    };
    if (byteLength(content) > FILE_LIMIT_BYTES) return { ...meta, error: "too_large" };
    if (used + byteLength(content) > RESPONSE_BUDGET_BYTES) {
      contentTruncated = true;
      return { ...meta, error: "budget" };
    }
    used += byteLength(content);
    return { ...meta, content };
  });
  return { files, content_truncated: contentTruncated };
}
