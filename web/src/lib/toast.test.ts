import { beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "./toast.svelte";

describe("toast", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  });

  it("shows a plain toast with no action", () => {
    const id = toast.success("Saved.");
    expect(toast.toasts.get(id)).toMatchObject({ message: "Saved.", kind: "success" });
    expect(toast.toasts.get(id)?.action).toBeUndefined();
  });

  it("carries an action through to the toast", () => {
    const onClick = vi.fn();
    const id = toast.success("Removed the key.", { label: "Undo", onClick });
    expect(toast.toasts.get(id)?.action).toEqual({ label: "Undo", onClick });
  });

  it("runAction invokes the action once and dismisses the toast", () => {
    const onClick = vi.fn();
    const id = toast.success("Removed the key.", { label: "Undo", onClick });

    toast.runAction(id);

    expect(onClick).toHaveBeenCalledTimes(1);
    expect(toast.toasts.has(id)).toBe(false);
  });

  it("runAction on an already-dismissed toast does nothing", () => {
    const onClick = vi.fn();
    const id = toast.success("Removed the key.", { label: "Undo", onClick });
    toast.dismiss(id);

    toast.runAction(id);

    expect(onClick).not.toHaveBeenCalled();
  });

  it("a toast with an action stays up longer than a plain one", () => {
    const plainId = toast.success("Saved.");
    const actionId = toast.success("Removed.", { label: "Undo", onClick: vi.fn() });

    vi.advanceTimersByTime(4001);
    expect(toast.toasts.has(plainId)).toBe(false);
    expect(toast.toasts.has(actionId)).toBe(true);

    vi.advanceTimersByTime(6001);
    expect(toast.toasts.has(actionId)).toBe(false);
  });

  it("an error toast never auto-dismisses", () => {
    const id = toast.error("Couldn't save.");
    vi.advanceTimersByTime(60_000);
    expect(toast.toasts.has(id)).toBe(true);
  });
});
