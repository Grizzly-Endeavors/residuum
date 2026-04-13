// ── Persistent fetch cache ──────────────────────────────────────────
//
// Generic cache + in-flight dedup for GET wrappers in api.ts.
// Backed by localStorage (so entries survive page reloads), with an
// in-memory fallback when storage is full or disabled.
//
// Keys are canonical request strings like "GET /api/config/raw" so each
// wrapper that opts in maps cleanly to a single entry.

const VERSION = "v1";
const NAMESPACE = `residuum:cache:${VERSION}:`;

const memoryFallback = new Map<string, unknown>();
const inflight = new Map<string, Promise<unknown>>();

function storageKey(key: string): string {
  return NAMESPACE + key;
}

function readPersisted(key: string): unknown {
  try {
    const raw = localStorage.getItem(storageKey(key));
    if (raw !== null) return JSON.parse(raw);
  } catch {
    // localStorage unavailable or value unparseable — fall through to memory
  }
  return memoryFallback.get(key);
}

function writePersisted(key: string, value: unknown): void {
  try {
    localStorage.setItem(storageKey(key), JSON.stringify(value));
    // Drop any in-memory copy so the two stores can't diverge (e.g. if an
    // earlier write had quota-failed and now we've recovered).
    memoryFallback.delete(key);
  } catch {
    // QuotaExceeded or storage disabled (private mode) — keep it in memory so
    // at least the current session benefits from the cache.
    memoryFallback.set(key, value);
  }
}

function deletePersisted(key: string): void {
  try {
    localStorage.removeItem(storageKey(key));
  } catch {
    // ignore
  }
  memoryFallback.delete(key);
}

/**
 * Returns a cached value for `key` if present, otherwise calls `fn`, stores
 * the result, and returns it. Concurrent calls with the same key share a
 * single in-flight Promise. Failed fetches are not cached.
 */
export async function cachedFetch<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const cached = readPersisted(key);
  if (cached !== undefined) return cached as T;

  const existing = inflight.get(key) as Promise<T> | undefined;
  if (existing) return existing;

  const p = fn()
    .then((value) => {
      writePersisted(key, value);
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
  deletePersisted(key);
  inflight.delete(key);
}
