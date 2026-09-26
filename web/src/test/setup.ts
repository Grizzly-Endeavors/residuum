import "@testing-library/jest-dom/vitest";
import { act, cleanup, setup as setupTestingLibrary } from "@testing-library/svelte";
import { afterEach, beforeEach, vi } from "vitest";

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
