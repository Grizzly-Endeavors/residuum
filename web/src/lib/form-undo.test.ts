import { beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { configCoordinator } from "./config-coordinator";
import { notifyFormUndo } from "./form-undo";
import { PendingSaveTracker } from "./pending-save";
import { toast } from "./toast.svelte";

let restore: MockInstance<typeof configCoordinator.restore>;

function lastToastAction(): { label: string; onClick: () => void } | undefined {
  return [...toast.toasts.values()].at(-1)?.action;
}

function errorToastMessage(): string | undefined {
  return [...toast.toasts.values()].find((t) => t.kind === "error")?.message;
}

describe("notifyFormUndo", () => {
  beforeEach(() => {
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
    restore = vi.spyOn(configCoordinator, "restore");
    restore.mockReset();
  });

  it("reverts local state and never touches the server when the save hasn't fired yet", () => {
    const pendingSave = new PendingSaveTracker();
    const revertLocally = vi.fn();

    notifyFormUndo(
      "scout",
      "Removed acme.",
      pendingSave,
      revertLocally,
      "agent_config",
      "providers.toml",
    );
    const timer = setTimeout(() => {}, 800);
    pendingSave.markScheduled(timer);
    lastToastAction()?.onClick();

    expect(revertLocally).toHaveBeenCalledTimes(1);
    expect(restore).not.toHaveBeenCalled();
    clearTimeout(timer);
  });

  it("restores from the checkpoint the removing save reported", async () => {
    const pendingSave = new PendingSaveTracker();
    restore.mockResolvedValue({ checkpoint_id: "cp-new", restored_paths: [] });
    const revertLocally = vi.fn();
    const onRestored = vi.fn();

    notifyFormUndo(
      "scout",
      "Removed acme.",
      pendingSave,
      revertLocally,
      "agent_config",
      "providers.toml",
      onRestored,
    );
    pendingSave.markSaving();
    pendingSave.recordWrite("providers.toml", "cp-removal");
    pendingSave.markSettled();
    // A later save's newer checkpoint must not be the one restored.
    pendingSave.markSaving();
    pendingSave.recordWrite("providers.toml", "cp-later");
    pendingSave.markSettled();
    lastToastAction()?.onClick();

    await vi.waitFor(() => {
      expect(onRestored).toHaveBeenCalledTimes(1);
    });
    expect(revertLocally).not.toHaveBeenCalled();
    expect(restore).toHaveBeenCalledWith("scout", "cp-removal", "agent_config", "providers.toml");
  });

  it("ignores a save that was already in flight when the entry was removed", async () => {
    const pendingSave = new PendingSaveTracker();
    pendingSave.markSaving();
    const revertLocally = vi.fn();

    notifyFormUndo(
      "scout",
      "Removed a server.",
      pendingSave,
      revertLocally,
      "workspace",
      "config/mcp.json",
    );
    pendingSave.recordWrite("config/mcp.json", "cp-before-removal");
    pendingSave.markSettled();
    lastToastAction()?.onClick();

    await vi.waitFor(() => {
      expect(revertLocally).toHaveBeenCalledTimes(1);
    });
    expect(restore).not.toHaveBeenCalled();
  });

  it("waits for an in-flight save to settle before restoring", async () => {
    const pendingSave = new PendingSaveTracker();
    restore.mockResolvedValue({ checkpoint_id: "cp2", restored_paths: [] });

    notifyFormUndo(
      "scout",
      "Removed a server.",
      pendingSave,
      vi.fn(),
      "workspace",
      "config/mcp.json",
    );
    pendingSave.markSaving();
    lastToastAction()?.onClick();

    // Still in flight — the restore must not have started yet.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(restore).not.toHaveBeenCalled();

    pendingSave.recordWrite("config/mcp.json", "cp1");
    pendingSave.markSettled();
    await vi.waitFor(() => {
      expect(restore).toHaveBeenCalledWith("scout", "cp1", "workspace", "config/mcp.json");
    });
  });

  it("reverts locally when the save that should have written the removal failed", async () => {
    const pendingSave = new PendingSaveTracker();
    const revertLocally = vi.fn();

    notifyFormUndo(
      "scout",
      "Removed acme.",
      pendingSave,
      revertLocally,
      "agent_config",
      "providers.toml",
    );
    pendingSave.markSaving();
    pendingSave.markSettled();
    lastToastAction()?.onClick();

    await vi.waitFor(() => {
      expect(revertLocally).toHaveBeenCalledTimes(1);
    });
    expect(restore).not.toHaveBeenCalled();
  });

  it("reports plainly when the save's checkpoint failed", async () => {
    const pendingSave = new PendingSaveTracker();

    notifyFormUndo(
      "scout",
      "Removed acme.",
      pendingSave,
      vi.fn(),
      "agent_config",
      "providers.toml",
    );
    pendingSave.markSaving();
    pendingSave.recordWrite("providers.toml", null);
    pendingSave.markSettled();
    lastToastAction()?.onClick();

    await vi.waitFor(() => {
      expect(errorToastMessage()).toContain("no checkpoint of providers.toml");
    });
    expect(restore).not.toHaveBeenCalled();
  });
});
