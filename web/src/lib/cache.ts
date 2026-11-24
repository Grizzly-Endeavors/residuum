const memory = new Map<string, unknown>();
const inflight = new Map<string, Promise<unknown>>();

const STALE_STORAGE_PREFIX = "residuum:cache:";

function clearStaleStorage(): void {
  try {
    const stale: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key?.startsWith(STALE_STORAGE_PREFIX)) stale.push(key);
    }
    for (const key of stale) localStorage.removeItem(key);
  } catch {}
}

clearStaleStorage();

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

export function invalidate(key: string): void {
  memory.delete(key);
  inflight.delete(key);
}
