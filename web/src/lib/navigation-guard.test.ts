// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { NavigationGuard } from "./navigation-guard";
import { HOME, locationAt } from "./routes";

describe("NavigationGuard", () => {
  it("finds nothing to lose with no checks", () => {
    expect(new NavigationGuard().losses(locationAt(HOME))).toEqual([]);
  });

  it("collects what each check says would be lost, leaving out the ones that lose nothing", () => {
    const guard = new NavigationGuard();
    guard.register(() => "an unsaved file");
    guard.register(() => null);
    guard.register(() => "staged settings");
    expect(guard.losses(locationAt(HOME))).toEqual(["an unsaved file", "staged settings"]);
  });

  it("hands each check the target, or null when the page is going away", () => {
    const guard = new NavigationGuard();
    const check = vi.fn(() => null);
    guard.register(check);
    const target = locationAt(HOME);
    guard.losses(target);
    guard.losses(null);
    expect(check.mock.calls).toEqual([[target], [null]]);
  });

  it("stops consulting a check once it is unregistered", () => {
    const guard = new NavigationGuard();
    const stop = guard.register(() => "an edit");
    stop();
    expect(guard.losses(null)).toEqual([]);
  });

  it("asks with the function it was given, and passes on the answer", async () => {
    const guard = new NavigationGuard();
    const confirm = vi.fn((_losses: readonly string[]) => Promise.resolve(true));
    guard.setConfirm(confirm);
    await expect(guard.ask(["an edit"])).resolves.toBe(true);
    expect(confirm).toHaveBeenCalledWith(["an edit"]);
    guard.setConfirm(() => Promise.resolve(false));
    await expect(guard.ask(["an edit"])).resolves.toBe(false);
  });

  it("declines when there is nothing to ask with", async () => {
    const guard = new NavigationGuard();
    await expect(guard.ask(["an edit"])).resolves.toBe(false);
    guard.setConfirm(() => Promise.resolve(true));
    guard.setConfirm(null);
    await expect(guard.ask(["an edit"])).resolves.toBe(false);
  });

  it("raises the browser's prompt for unload only while something would be lost", () => {
    const guard = new NavigationGuard();
    const clean = new Event("beforeunload", { cancelable: true }) as BeforeUnloadEvent;
    guard.onBeforeUnload(clean);
    expect(clean.defaultPrevented).toBe(false);

    guard.register((target) => (target === null ? "an edit" : null));
    const dirty = new Event("beforeunload", { cancelable: true }) as BeforeUnloadEvent;
    guard.onBeforeUnload(dirty);
    expect(dirty.defaultPrevented).toBe(true);
  });

  describe("a reload the app starts itself", () => {
    const unload = (guard: NavigationGuard): boolean => {
      const event = new Event("beforeunload", { cancelable: true }) as BeforeUnloadEvent;
      guard.onBeforeUnload(event);
      return event.defaultPrevented;
    };

    it("goes ahead without asking when nothing would be lost", async () => {
      const guard = new NavigationGuard();
      const confirm = vi.fn((_losses: readonly string[]) => Promise.resolve(true));
      guard.setConfirm(confirm);
      await expect(guard.confirmReload()).resolves.toBe(true);
      expect(confirm).not.toHaveBeenCalled();
    });

    it("asks what the reload would lose, and stops when the person keeps editing", async () => {
      const guard = new NavigationGuard();
      guard.register((target) => (target === null ? "an unsaved file" : null));
      const confirm = vi.fn((_losses: readonly string[]) => Promise.resolve(false));
      guard.setConfirm(confirm);
      await expect(guard.confirmReload()).resolves.toBe(false);
      expect(confirm).toHaveBeenCalledWith(["an unsaved file"]);
      // Still guarded: the person's own reload gets the browser's prompt.
      expect(unload(guard)).toBe(true);
    });

    it("stops the browser's prompt from asking a second time once the person has agreed", async () => {
      const guard = new NavigationGuard();
      guard.register((target) => (target === null ? "an unsaved file" : null));
      guard.setConfirm(() => Promise.resolve(true));
      await expect(guard.confirmReload()).resolves.toBe(true);
      expect(unload(guard)).toBe(false);
    });

    it("refuses when something would be lost and there is no way to ask", async () => {
      const guard = new NavigationGuard();
      guard.register(() => "an unsaved file");
      await expect(guard.confirmReload()).resolves.toBe(false);
    });
  });
});
