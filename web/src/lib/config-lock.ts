// ── Config write lock ────────────────────────────────────────────────
//
// Serializes read-modify-write operations on providers.toml to prevent
// concurrent saves from clobbering each other.

let pending: Promise<void> = Promise.resolve();

export function withConfigLock<T>(fn: () => Promise<T>): Promise<T> {
  const next = pending.then(fn, fn);
  pending = next.then(
    () => {},
    () => {},
  );
  return next;
}
