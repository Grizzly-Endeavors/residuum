/** Ascending string order, for listing agents and other named records. */
export function byName(a: string, b: string): number {
  if (a === b) return 0;
  return a < b ? -1 : 1;
}

/** Resolve after `ms` milliseconds. */
export function sleep(ms: number): Promise<void> {
  return new Promise<void>((done) => {
    setTimeout(done, ms);
  });
}
