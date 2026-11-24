import type { WorkspaceChange } from "./types";

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

/**
 * Normalize a prefix for the hub connection's `watch_team`: `team` or a path
 * under `team/` (`team/wiki`), the spelling the hub's change feed uses.
 * Returns `null` for anything else, which the hub would refuse.
 */
export function normalizeTeamWatchPrefix(prefix: string): string | null {
  const normalized = normalizeWatchPrefix(prefix);
  if (normalized === null) return null;
  return normalized === "team" || normalized.startsWith("team/") ? normalized : null;
}

function isWithin(path: string, ancestor: string): boolean {
  return (
    ancestor === "" ||
    path === ancestor ||
    (path.startsWith(ancestor) && path.charAt(ancestor.length) === "/")
  );
}

export function changeMatchesPrefix(path: string, prefix: string): boolean {
  return isWithin(path, prefix) || isWithin(prefix, path);
}

export function changesUnder(
  changes: WorkspaceChange[],
  prefixes: readonly string[],
): WorkspaceChange[] {
  return changes.filter((change) => prefixes.some((p) => changeMatchesPrefix(change.path, p)));
}
