import { describe, expect, it, vi } from "vitest";
import { showAppBadge } from "./app-badge";

describe("the app icon's badge", () => {
  it("shows the count, and clears at zero", () => {
    const nav = {
      setAppBadge: vi.fn(() => Promise.resolve()),
      clearAppBadge: vi.fn(() => Promise.resolve()),
    };
    showAppBadge(3, nav);
    showAppBadge(0, nav);
    expect(nav.setAppBadge).toHaveBeenCalledWith(3);
    expect(nav.clearAppBadge).toHaveBeenCalledOnce();
  });

  it("does nothing where the browser has no badge", () => {
    expect(() => {
      showAppBadge(3, {});
    }).not.toThrow();
  });

  it("takes a refusal quietly", async () => {
    // An unhandled rejection would fail the run.
    showAppBadge(3, { setAppBadge: () => Promise.reject(new Error("not installed")) });
    showAppBadge(0, { clearAppBadge: () => Promise.reject(new Error("not installed")) });
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});
