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
    notifyWithUndo("Removed github_token.", "hub", "agent-keys.toml.enc", "action-cp");
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
    notifyWithUndo("Removed github_token.", "hub", "agent-keys.toml.enc", "action-cp", onRestored);
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await vi.waitFor(() => {
      expect(onRestored).toHaveBeenCalledTimes(1);
    });

    expect(undoLastAction).toHaveBeenCalledWith("action-cp", "hub", "agent-keys.toml.enc");
    const followUp = [...toast.toasts.values()].at(-1);
    expect(followUp?.message).toBe("Restored.");
  });

  it("surfaces a plain-language error if the restore call itself fails", async () => {
    undoLastAction.mockRejectedValue(new Error("network down"));
    notifyWithUndo("Removed github_token.", "hub", "agent-keys.toml.enc", "action-cp");
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await vi.waitFor(() => {
      const followUp = [...toast.toasts.values()].at(-1);
      expect(followUp?.kind).toBe("error");
    });
  });

  it("restores every listed path from the same checkpoint", async () => {
    undoLastAction.mockResolvedValue({ checkpoint_id: "abc123", restored_paths: [] });
    notifyWithUndo(
      'Deleted "Chart".',
      "team",
      ["workbench/chart.html", "workbench/chart.state.json"],
      "action-cp",
    );

    [...toast.toasts.values()].at(-1)?.action?.onClick();
    await vi.waitFor(() => {
      expect(undoLastAction).toHaveBeenCalledTimes(2);
    });

    expect(undoLastAction).toHaveBeenNthCalledWith(1, "action-cp", "team", "workbench/chart.html");
    expect(undoLastAction).toHaveBeenNthCalledWith(
      2,
      "action-cp",
      "team",
      "workbench/chart.state.json",
    );
  });
});
