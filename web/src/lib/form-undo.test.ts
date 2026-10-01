import { beforeEach, describe, expect, it, vi } from "vitest";
import { notifyStagedRemoval } from "./form-undo";
import { toast } from "./toast.svelte";

describe("notifyStagedRemoval", () => {
  beforeEach(() => {
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  });

  it("says what was removed, and its Undo puts the entry back", () => {
    const putBack = vi.fn();

    notifyStagedRemoval("Removed acme.", putBack);
    const shown = [...toast.toasts.values()].at(-1);
    expect(shown?.message).toBe("Removed acme.");
    expect(shown?.action?.label).toBe("Undo");

    shown?.action?.onClick();
    expect(putBack).toHaveBeenCalledTimes(1);
  });
});
