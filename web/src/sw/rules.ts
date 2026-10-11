// The service worker's rules as pure functions, so they are tested without a
// worker: which requests it answers and how, and which caches it keeps.
//
// The worker caches the app shell and nothing else. No data is cached for
// offline reading, and a request for the hub's API, its sockets, a webhook or
// the cloud sign-in callback never reaches a cache or is answered by the
// worker at all.

/** The app's document, which every client route is served from. */
export const SHELL_URL = "/index.html";

/** Build output named by content hash: scripts, styles and fonts. A file here never changes under its name. */
const HASHED_DIR = "/assets/";

/** First path segments the hub serves itself. They are the hub's to answer, never the app shell's. */
const HUB_SEGMENTS: ReadonlySet<string> = new Set(["api", "ws", "webhook", "cloud"]);

/** Every cache of a shell version starts with this, so housekeeping never touches another cache. */
const SHELL_CACHE_PREFIX = "residuum-shell:";

/** How the worker answers a request. */
export type Handling =
  /** Not the worker's: the browser sends it as it would with no worker. */
  | "pass"
  /** A page load: the network first, the cached app shell when the hub can't be reached. */
  | "navigate"
  /** A file of the app: the cache first, the network when it isn't there. */
  | "cache-first";

/** The parts of a request the rules read. */
export interface RequestFacts {
  method: string;
  url: string;
  /** The request's mode: `navigate` for a page load. */
  mode: string;
}

/** Whether `path` is build output named by content hash. */
export function isHashedAsset(path: string): boolean {
  return path.startsWith(HASHED_DIR);
}

function firstSegment(path: string): string {
  return path.split("/")[1] ?? "";
}

/**
 * Decide how to answer `request`. `precached` is the worker's own list of
 * files outside `/assets/` (the document, the icons).
 */
export function handlingOf(
  request: RequestFacts,
  origin: string,
  precached: ReadonlySet<string>,
): Handling {
  if (request.method !== "GET") return "pass";
  const url = new URL(request.url);
  if (url.origin !== origin || HUB_SEGMENTS.has(firstSegment(url.pathname))) return "pass";
  if (request.mode === "navigate") {
    // A path with a dot names a file, which the hub serves or answers 404 itself.
    return url.pathname === SHELL_URL || !url.pathname.includes(".") ? "navigate" : "pass";
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

/** The name of the cache holding one shell version's files. */
export function shellCacheName(version: string): string {
  return `${SHELL_CACHE_PREFIX}${version}`;
}

/** The shell versions that have been active: the current one, and the one it replaced. */
export interface Generations {
  current: string;
  previous: string | null;
}

/**
 * What to record when `version` activates, given what was recorded before. The
 * version it replaces stays, since a page opened under it keeps asking for the
 * hashed files it was built with, and a new deploy must not break those.
 */
export function recordActivation(recorded: Generations | null, version: string): Generations {
  if (recorded === null) return { current: version, previous: null };
  if (recorded.current === version) return recorded;
  return { current: version, previous: recorded.current };
}

/** Read back what `recordActivation` produced, or null for anything else. */
export function parseGenerations(value: unknown): Generations | null {
  if (typeof value !== "object" || value === null) return null;
  const current = "current" in value ? value.current : undefined;
  const previous = "previous" in value ? value.previous : undefined;
  if (typeof current !== "string") return null;
  if (previous !== null && typeof previous !== "string") return null;
  return { current, previous };
}

/** The shell caches to delete: every version but the generations to keep. Other caches are never listed. */
export function staleCaches(existing: readonly string[], keep: Generations): string[] {
  const kept = new Set([shellCacheName(keep.current)]);
  if (keep.previous !== null) kept.add(shellCacheName(keep.previous));
  return existing.filter((name) => name.startsWith(SHELL_CACHE_PREFIX) && !kept.has(name));
}
