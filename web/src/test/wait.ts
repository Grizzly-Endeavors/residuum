import { vi } from "vitest";
import { QUERY_GUARD_MS } from "./guard";

/**
 * Run `check` until it stops throwing, under the hang guard (./guard.ts). Use
 * this rather than `vi.waitFor`, whose one-second default fails on a slow
 * machine.
 */
export function waitFor<T>(check: () => T | Promise<T>): Promise<T> {
  return vi.waitFor(check, { timeout: QUERY_GUARD_MS, interval: 20 });
}
