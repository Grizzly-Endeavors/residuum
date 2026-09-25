import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as ApiModule from "./api";
import { toast } from "./toast.svelte";
import { notifyWithUndo } from "./undo";

const { undoLastAction } = vi.hoisted(() => ({ undoLastAction: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof ApiModule>()),
  undoLastAction,
}));

describe("notifyWithUndo", () => {
  beforeEach(() => {
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
    undoLastAction.mockReset();
  });

  it("shows a success toast with an Undo action", () => {
    notifyWithUndo("Removed github_token.", "config", "agent-keys.toml.enc");
    const shown = [...toast.toasts.values()].at(-1);
    expect(shown).toMatchObject({ kind: "success", message: "Removed github_token." });
    expect(shown?.action?.label).toBe("Undo");
  });

  it("restores from the checkpoint and reports success when Undo is clicked", async () => {
    undoLastAction.mockResolvedValue({
      checkpoint_id: "abc123",
      restored_paths: ["agent-keys.toml.enc"],
    });
    const onRestored = vi.fn();
    notifyWithUndo("Removed github_token.", "config", "agent-keys.toml.enc", onRestored);
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await vi.waitFor(() => {
      expect(onRestored).toHaveBeenCalledTimes(1);
    });

    expect(undoLastAction).toHaveBeenCalledWith("config", "agent-keys.toml.enc");
    const followUp = [...toast.toasts.values()].at(-1);
    expect(followUp?.message).toBe("Restored.");
  });

  it("reports plainly when there is no checkpoint to restore from", async () => {
    undoLastAction.mockResolvedValue(null);
    notifyWithUndo("Removed github_token.", "config", "agent-keys.toml.enc");
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await vi.waitFor(() => {
      const followUp = [...toast.toasts.values()].at(-1);
      expect(followUp?.kind).toBe("error");
    });

    const followUp = [...toast.toasts.values()].at(-1);
    expect(followUp?.message).toContain("Nothing to restore");
  });

  it("surfaces a plain-language error if the restore call itself fails", async () => {
    undoLastAction.mockRejectedValue(new Error("network down"));
    notifyWithUndo("Removed github_token.", "config", "agent-keys.toml.enc");
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await vi.waitFor(() => {
      const followUp = [...toast.toasts.values()].at(-1);
      expect(followUp?.kind).toBe("error");
    });
  });
});
