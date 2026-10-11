// ── In-memory fetch cache ───────────────────────────────────────────
//
// Generic cache + in-flight dedup for GET wrappers in api.ts.
// Keys are canonical request strings like "GET /api/config/raw" so each
// wrapper that opts in maps cleanly to a single entry.
//
// Entries live for the page's lifetime only, never in browser storage. One
// web address can reach several instances (the relay sends a user's address
// to whichever instance is active), and an agent can be deleted and created
// again under the same name, so nothing a request answered can be trusted
// on a later page load: a persisted entry would answer one instance's
// request with another's data. Switching instances reloads the page, which
// starts the cache empty.

const memory = new Map<string, unknown>();
const inflight = new Map<string, Promise<unknown>>();

// Entries under this prefix in localStorage hold another page load's
// answers, possibly another instance's config text. Nothing reads them, so
// they are removed when the app starts.
const STALE_STORAGE_PREFIX = "residuum:cache:";

function clearStaleStorage(): void {
  try {
    const stale: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key?.startsWith(STALE_STORAGE_PREFIX)) stale.push(key);
    }
    for (const key of stale) localStorage.removeItem(key);
  } catch {
    // Storage unavailable (private mode, a test environment): nothing to clear.
  }
}

clearStaleStorage();

/**
 * Returns a cached value for `key` if present, otherwise calls `fn`, stores
 * the result, and returns it. Concurrent calls with the same key share a
 * single in-flight Promise. Failed fetches are not cached.
 */
export async function cachedFetch<T>(key: string, fn: () => Promise<T>): Promise<T> {
  if (memory.has(key)) return memory.get(key) as T;

  const existing = inflight.get(key) as Promise<T> | undefined;
  if (existing) return existing;

  const p = fn()
    .then((value) => {
      memory.set(key, value);
      inflight.delete(key);
      return value;
    })
    .catch((err: unknown) => {
      inflight.delete(key);
      throw err;
    });

  inflight.set(key, p);
  return p;
}

/** Drop a single cache entry (and any in-flight Promise for it). */
export function invalidate(key: string): void {
  memory.delete(key);
  inflight.delete(key);
}
