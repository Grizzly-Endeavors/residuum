import type { WorkspaceEntry } from "../src/lib/types";
import type { MockState } from "./state";
import { byName } from "./util";

/**
 * The workspace files of one state, as the workspace API sees them: file
 * contents by path in `workspaceFileContents`, and each directory's listing
 * in `workspaceFiles`. Paths are `/`-separated and relative to the workspace;
 * `""` is its root. Every change goes through these functions, so a listing
 * always agrees with the files and with the version a read reports.
 */

/** A hash of `text`, as lowercase hex. */
function hashHex(text: string): string {
  let hash = 0;
  for (let i = 0; i < text.length; i++) hash = (hash * 31 + text.charCodeAt(i)) >>> 0;
  return hash.toString(16);
}

/** The size of `content` on disk. */
export function byteLength(content: string): number {
  return Buffer.byteLength(content);
}

/**
 * A file's version token: the `ETag` of a read, the `version` of a listing
 * entry and of a write. Like the backend's, it is opaque and changes with the
 * content.
 */
export function fileVersion(content: string): string {
  return `${hashHex(content)}-${byteLength(content).toString(16)}`;
}

/** A directory's version token, which changes whenever its modification time does. */
export function dirEntryVersion(path: string, modified: number): string {
  return `${hashHex(path)}-${modified.toString(16)}`;
}

/** The directory holding `path`. */
function parentOf(path: string): string {
  return path.slice(0, Math.max(path.lastIndexOf("/"), 0));
}

function nameOf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** What `path` is in the workspace, or `null` when nothing is there. */
export function pathKind(state: MockState, path: string): "file" | "directory" | null {
  if (path in state.workspaceFileContents) return "file";
  if (path === "" || path in state.workspaceFiles) return "directory";
  return null;
}

/** A directory's entries, directories first and each group by name; `undefined` when it doesn't exist. */
export function listDirectory(state: MockState, path: string): WorkspaceEntry[] | undefined {
  const entries = state.workspaceFiles[path];
  if (entries === undefined) return undefined;
  const rank = (entry: WorkspaceEntry): number => (entry.entry_type === "directory" ? 0 : 1);
  return [...entries].sort((a, b) => rank(a) - rank(b) || byName(a.name, b.name));
}

/** A modification time later than `previous`, so a change is never mistaken for none. */
function nextModified(previous: number): number {
  return Math.max(Date.now(), previous + 1);
}

function upsertEntry(state: MockState, dir: string, entry: WorkspaceEntry): void {
  const entries = (state.workspaceFiles[dir] ??= []);
  const at = entries.findIndex((existing) => existing.name === entry.name);
  if (at === -1) entries.push(entry);
  else entries[at] = entry;
}

/** Record that `dir` changed: its entry in its parent's listing gets a new modification time and version. */
function touchDirectory(state: MockState, dir: string): void {
  if (dir === "") return;
  const entry = state.workspaceFiles[parentOf(dir)]?.find((e) => e.name === nameOf(dir));
  if (entry === undefined) return;
  entry.modified = nextModified(entry.modified);
  entry.version = dirEntryVersion(dir, entry.modified);
}

/** Create `dir` and any missing parents. */
export function ensureDirectory(state: MockState, dir: string): void {
  if (pathKind(state, dir) !== null) return;
  const parent = parentOf(dir);
  ensureDirectory(state, parent);
  state.workspaceFiles[dir] = [];
  const modified = Date.now();
  upsertEntry(state, parent, {
    name: nameOf(dir),
    entry_type: "directory",
    size: null,
    modified,
    version: dirEntryVersion(dir, modified),
  });
  touchDirectory(state, parent);
}

/** Write a file, creating its directories, and report its new version. */
export function writeFile(state: MockState, path: string, content: string): string {
  const dir = parentOf(path);
  ensureDirectory(state, dir);
  state.workspaceFileContents[path] = content;
  const version = fileVersion(content);
  upsertEntry(state, dir, {
    name: nameOf(path),
    entry_type: "file",
    size: byteLength(content),
    modified: Date.now(),
    version,
  });
  touchDirectory(state, dir);
  return version;
}

/** Remove a file, or a directory with everything in it. */
export function removePath(state: MockState, path: string): void {
  const dir = parentOf(path);
  const listing = state.workspaceFiles[dir];
  if (listing !== undefined) {
    state.workspaceFiles[dir] = listing.filter((entry) => entry.name !== nameOf(path));
  }
  const inside = `${path}/`;
  for (const file of Object.keys(state.workspaceFileContents)) {
    if (file === path || file.startsWith(inside)) delete state.workspaceFileContents[file];
  }
  for (const directory of Object.keys(state.workspaceFiles)) {
    if (directory === path || directory.startsWith(inside)) delete state.workspaceFiles[directory];
  }
  touchDirectory(state, dir);
}

/**
 * Move a file, or a directory with everything in it, to `to`, in the same
 * state or another (the team tree lives in the hub's). What was at `to` is
 * replaced.
 */
export function movePath(from: MockState, fromPath: string, to: MockState, toPath: string): void {
  const inside = `${fromPath}/`;
  const files = Object.entries(from.workspaceFileContents).filter(
    ([file]) => file === fromPath || file.startsWith(inside),
  );
  const directories = Object.keys(from.workspaceFiles).filter((dir) => dir.startsWith(inside));
  const isDirectory = pathKind(from, fromPath) === "directory";

  removePath(from, fromPath);
  if (pathKind(to, toPath) !== null) removePath(to, toPath);
  if (isDirectory) ensureDirectory(to, toPath);
  for (const dir of directories) ensureDirectory(to, toPath + dir.slice(fromPath.length));
  for (const [file, content] of files) writeFile(to, toPath + file.slice(fromPath.length), content);
}
