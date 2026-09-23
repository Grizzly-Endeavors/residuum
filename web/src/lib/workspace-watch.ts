// ── Workspace change feed (client side) ──────────────────────────────
//
// The gateway keeps one watch set per WebSocket connection: the workspace
// path prefixes the connection wants `workspace_changed` frames for. The web
// UI shows one artifact at a time, so the connection's set is the open
// artifact's prefixes, empty when none is open. A new connection starts with
// an empty set, so the set is sent again after every reconnect.

import type { ClientMessage, WorkspaceChange } from "./types";

/**
 * Normalize a workspace path prefix the way the gateway does: `/`-separated,
 * no empty or `.` segments, `""` for the whole workspace. Returns `null` for
 * a prefix the gateway would refuse (absolute, `..`, a backslash or NUL).
 */
export function normalizeWatchPrefix(prefix: string): string | null {
  if (prefix.includes("\\") || prefix.includes("\0")) return null;
  const segments = prefix.split("/");
  if (prefix.startsWith("/") || (segments[0] ?? "").includes(":")) return null;
  const kept: string[] = [];
  for (const segment of segments) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") return null;
    kept.push(segment);
  }
  return kept.join("/");
}

/** Whether `path` is `ancestor` or lies under it, by whole path segments. */
function isWithin(path: string, ancestor: string): boolean {
  return (
    ancestor === "" ||
    path === ancestor ||
    (path.startsWith(ancestor) && path.charAt(ancestor.length) === "/")
  );
}

/**
 * Whether a change at `path` concerns `prefix`, matching the gateway: the
 * prefix itself, anything under it (so `wiki` never matches `wikipedia`), or
 * a directory containing it.
 */
export function changeMatchesPrefix(path: string, prefix: string): boolean {
  return isWithin(path, prefix) || isWithin(prefix, path);
}

/** The changes that concern any of `prefixes`. */
export function changesUnder(
  changes: WorkspaceChange[],
  prefixes: readonly string[],
): WorkspaceChange[] {
  return changes.filter((change) => prefixes.some((p) => changeMatchesPrefix(change.path, p)));
}

/** Keeps the connection's watch set in step with the open artifact. */
export class WorkspaceWatchSync {
  private prefixes: string[] = [];

  constructor(private readonly send: (msg: ClientMessage) => void) {}

  /** The prefixes the connection should watch now. */
  get current(): readonly string[] {
    return this.prefixes;
  }

  /** Replace the watched prefixes, telling the gateway only when they change. */
  set(prefixes: readonly string[]): void {
    const next = [...new Set(prefixes)].sort();
    if (next.length === this.prefixes.length && next.every((p, i) => p === this.prefixes[i])) {
      return;
    }
    this.prefixes = next;
    this.send({ type: "watch_workspace", prefixes: next });
  }

  /** A connection opened: it watches nothing until told again. */
  connected(): void {
    if (this.prefixes.length > 0) this.send({ type: "watch_workspace", prefixes: this.prefixes });
  }
}
