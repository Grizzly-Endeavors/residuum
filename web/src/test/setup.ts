import "@testing-library/jest-dom/vitest";
import { act, cleanup, configure, setup as setupTestingLibrary } from "@testing-library/svelte";
import { afterEach, beforeEach, vi } from "vitest";
import { QUERY_GUARD_MS } from "./guard";

// `findBy*` and `waitFor` end when their element shows up; the bound only
// catches one that never does (see ./guard.ts).
configure({ asyncUtilTimeout: QUERY_GUARD_MS });

/**
 * jsdom lays nothing out, and has no ResizeObserver. A component that watches
 * sizes gets one that never reports; a test that needs sizes stubs its own.
 */
class SilentResizeObserver implements ResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

if (!("ResizeObserver" in globalThis)) globalThis.ResizeObserver = SilentResizeObserver;

beforeEach(async () => {
  await setupTestingLibrary();
});

afterEach(async () => {
  // Real timers before unmount: a component test may have faked the clock
  // (the update page polls for 90s), and cleanup has to run against the
  // real event loop or the interval from the previous test leaks.
  if (vi.isFakeTimers()) {
    vi.clearAllTimers();
    vi.useRealTimers();
  }
  await act();
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});
