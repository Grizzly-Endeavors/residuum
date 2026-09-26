import { describe, expect, it } from "vitest";
import { PendingSaveTracker } from "./pending-save";

describe("PendingSaveTracker", () => {
  it("cancels a save that hasn't started yet", () => {
    const tracker = new PendingSaveTracker();
    const timer = setTimeout(() => {}, 800);
    tracker.markScheduled(timer);

    expect(tracker.cancelIfPending()).toBe(true);
    clearTimeout(timer);
  });

  it("refuses to cancel once the save has started", () => {
    const tracker = new PendingSaveTracker();
    const timer = setTimeout(() => {}, 800);
    tracker.markScheduled(timer);
    tracker.markSaving();

    expect(tracker.cancelIfPending()).toBe(false);
    clearTimeout(timer);
  });

  it("refuses to cancel once the save has settled", () => {
    const tracker = new PendingSaveTracker();
    const timer = setTimeout(() => {}, 800);
    tracker.markScheduled(timer);
    tracker.markSaving();
    tracker.markSettled();

    expect(tracker.cancelIfPending()).toBe(false);
    clearTimeout(timer);
  });

  it("refuses to cancel when nothing was ever scheduled", () => {
    const tracker = new PendingSaveTracker();
    expect(tracker.cancelIfPending()).toBe(false);
  });

  it("waitForSettled resolves immediately when nothing is saving", async () => {
    const tracker = new PendingSaveTracker();
    await expect(tracker.waitForSettled()).resolves.toBeUndefined();
  });

  it("waitForSettled resolves once markSettled is called", async () => {
    const tracker = new PendingSaveTracker();
    tracker.markSaving();

    let resolved = false;
    const wait = tracker.waitForSettled().then(() => {
      resolved = true;
    });

    expect(resolved).toBe(false);
    tracker.markSettled();
    await wait;
    expect(resolved).toBe(true);
  });

  it("waitForSettled resolves every concurrent waiter", async () => {
    const tracker = new PendingSaveTracker();
    tracker.markSaving();

    const waits = [tracker.waitForSettled(), tracker.waitForSettled(), tracker.waitForSettled()];
    tracker.markSettled();

    await expect(Promise.all(waits)).resolves.toEqual([undefined, undefined, undefined]);
  });

  it("finds the first write to a file by a save started after the mark", () => {
    const tracker = new PendingSaveTracker();
    tracker.markSaving();
    tracker.recordWrite("providers.toml", "before-mark");
    const mark = tracker.mark();
    tracker.recordWrite("providers.toml", "same-save-as-mark");
    tracker.markSettled();

    tracker.markSaving();
    tracker.recordWrite("config.toml", "other-file");
    tracker.recordWrite("providers.toml", "cp-after");
    tracker.markSettled();
    tracker.markSaving();
    tracker.recordWrite("providers.toml", "cp-later");
    tracker.markSettled();

    expect(tracker.firstWriteAfter(mark, "providers.toml")).toEqual({
      kind: "written",
      checkpointId: "cp-after",
    });
  });

  it("reports not-written when no later save touched the file", () => {
    const tracker = new PendingSaveTracker();
    const mark = tracker.mark();
    tracker.markSaving();
    tracker.recordWrite("config.toml", "cp1");
    tracker.markSettled();

    expect(tracker.firstWriteAfter(mark, "providers.toml")).toEqual({ kind: "not-written" });
  });

  it("keeps a write whose checkpoint failed, with a null id", () => {
    const tracker = new PendingSaveTracker();
    const mark = tracker.mark();
    tracker.markSaving();
    tracker.recordWrite("config/mcp.json", null);
    tracker.markSettled();

    expect(tracker.firstWriteAfter(mark, "config/mcp.json")).toEqual({
      kind: "written",
      checkpointId: null,
    });
  });
});
