/** Ascending string order, for listing agents and other named records. */
export function byName(a: string, b: string): number {
  if (a === b) return 0;
  return a < b ? -1 : 1;
}
