export const SHELL_URL = "/index.html";

const HASHED_DIR = "/assets/";

const HUB_SEGMENTS: ReadonlySet<string> = new Set(["api", "ws", "webhook", "cloud"]);

const SHELL_CACHE_PREFIX = "residuum-shell:";

export type Handling = "pass" | "navigate" | "cache-first";

export interface RequestFacts {
  method: string;
  url: string;
  mode: string;
}

export function isHashedAsset(path: string): boolean {
  return path.startsWith(HASHED_DIR);
}

function firstSegment(path: string): string {
  return path.split("/")[1] ?? "";
}

export function handlingOf(
  request: RequestFacts,
  origin: string,
  precached: ReadonlySet<string>,
): Handling {
  if (request.method !== "GET") return "pass";
  const url = new URL(request.url);
  if (url.origin !== origin || HUB_SEGMENTS.has(firstSegment(url.pathname))) return "pass";
  if (request.mode === "navigate") {
    if (url.pathname === SHELL_URL) return "navigate";
    return url.pathname.includes(".") && precached.has(url.pathname) ? "cache-first" : "navigate";
  }
  return isHashedAsset(url.pathname) || precached.has(url.pathname) ? "cache-first" : "pass";
}

/**
 * Whether a page load's answer says the hub isn't there. Residuum Cloud's
 * relay answers 503 for an instance that is offline and 504 for one that
 * doesn't answer, and a reverse proxy in front of a hub that is down answers
 * 502 or 503. The shell and its banner explain that better than the bare
 * error page.
 */
export function isGatewayFailure(status: number): boolean {
  return status === 502 || status === 503 || status === 504;
}

export function shellCacheName(version: string): string {
  return `${SHELL_CACHE_PREFIX}${version}`;
}

export interface Generations {
  current: string;
  previous: string | null;
}

export function recordActivation(recorded: Generations | null, version: string): Generations {
  if (recorded === null) return { current: version, previous: null };
  if (recorded.current === version) return recorded;
  return { current: version, previous: recorded.current };
}

export function parseGenerations(value: unknown): Generations | null {
  if (typeof value !== "object" || value === null) return null;
  const current = "current" in value ? value.current : undefined;
  const previous = "previous" in value ? value.previous : undefined;
  if (typeof current !== "string") return null;
  if (previous !== null && typeof previous !== "string") return null;
  return { current, previous };
}

export function staleCaches(existing: readonly string[], keep: Generations): string[] {
  const kept = new Set([shellCacheName(keep.current)]);
  if (keep.previous !== null) kept.add(shellCacheName(keep.previous));
  return existing.filter((name) => name.startsWith(SHELL_CACHE_PREFIX) && !kept.has(name));
}
