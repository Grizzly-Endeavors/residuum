import { vi } from "vitest";
import { QUERY_GUARD_MS } from "./guard";

export function waitFor<T>(check: () => T | Promise<T>): Promise<T> {
  return vi.waitFor(check, { timeout: QUERY_GUARD_MS, interval: 20 });
}
