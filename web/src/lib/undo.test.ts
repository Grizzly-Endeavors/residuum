import { beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { parseWorkspaceCheckpoints } from "./api";
import { configCoordinator } from "./config-coordinator";
import { toast } from "./toast.svelte";
import { notifyWithUndo, notifyWithWorkspaceUndo, restoreTargets } from "./undo";
import { waitFor } from "../test/wait";

let restore: MockInstance<typeof configCoordinator.restore>;

describe("notifyWithUndo", () => {
  beforeEach(() => {
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
    restore = vi.spyOn(configCoordinator, "restore");
    restore.mockReset();
  });

  it("shows a success toast with an Undo action", () => {
    notifyWithUndo(null, "Removed github_token.", "hub", "agent-keys.toml.enc", "action-cp");
    const shown = [...toast.toasts.values()].at(-1);
    expect(shown).toMatchObject({ kind: "success", message: "Removed github_token." });
    expect(shown?.action?.label).toBe("Undo");
  });

  it("restores from the checkpoint and reports success when Undo is clicked", async () => {
    restore.mockResolvedValue({
      checkpoint_id: "abc123",
      restored_paths: ["agent-keys.toml.enc"],
    });
    const onRestored = vi.fn();
    notifyWithUndo(
      null,
      "Removed github_token.",
      "hub",
      "agent-keys.toml.enc",
      "action-cp",
      onRestored,
    );
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await waitFor(() => {
      expect(onRestored).toHaveBeenCalledTimes(1);
    });

    expect(restore).toHaveBeenCalledWith(null, "action-cp", "hub", "agent-keys.toml.enc");
    const followUp = [...toast.toasts.values()].at(-1);
    expect(followUp?.message).toBe("Restored.");
  });

  it("surfaces a plain-language error if the restore call itself fails", async () => {
    restore.mockRejectedValue(new Error("network down"));
    notifyWithUndo(null, "Removed github_token.", "hub", "agent-keys.toml.enc", "action-cp");
    const shown = [...toast.toasts.values()].at(-1);

    shown?.action?.onClick();
    await waitFor(() => {
      const followUp = [...toast.toasts.values()].at(-1);
      expect(followUp?.kind).toBe("error");
    });
  });

  it("restores every listed path from the same checkpoint", async () => {
    restore.mockResolvedValue({ checkpoint_id: "abc123", restored_paths: [] });
    notifyWithUndo(
      null,
      'Deleted "Chart".',
      "team",
      ["workbench/chart.html", "workbench/chart.state.json"],
      "action-cp",
    );

    [...toast.toasts.values()].at(-1)?.action?.onClick();
    await waitFor(() => {
      expect(restore).toHaveBeenCalledTimes(2);
    });

    expect(restore).toHaveBeenNthCalledWith(1, null, "action-cp", "team", "workbench/chart.html");
    expect(restore).toHaveBeenNthCalledWith(
      2,
      null,
      "action-cp",
      "team",
      "workbench/chart.state.json",
    );
  });
});

describe("restoreTargets", () => {
  const workspaceCp = { id: "ws-cp", repo: "workspace" as const };
  const teamCp = { id: "team-cp", repo: "team" as const };

  it("restores an agent path from the workspace checkpoint", () => {
    expect(restoreTargets(["notes/a.md"], [workspaceCp])).toEqual([
      { checkpointId: "ws-cp", repo: "workspace", path: "notes/a.md" },
    ]);
  });

  it("restores a team path from the team checkpoint, relative to team/", () => {
    expect(restoreTargets(["team/wiki/a.md"], [teamCp])).toEqual([
      { checkpointId: "team-cp", repo: "team", path: "wiki/a.md" },
    ]);
  });

  it("picks each path's own repo when both checkpoints exist", () => {
    expect(restoreTargets(["draft.md", "team/wiki/page.md"], [workspaceCp, teamCp])).toEqual([
      { checkpointId: "ws-cp", repo: "workspace", path: "draft.md" },
      { checkpointId: "team-cp", repo: "team", path: "wiki/page.md" },
    ]);
  });

  it("leaves out a path whose repo recorded no checkpoint", () => {
    expect(restoreTargets(["team/a.md"], [workspaceCp])).toEqual([]);
  });

  it("does not treat a path merely starting with team as a team path", () => {
    expect(restoreTargets(["teamwork.md"], [workspaceCp])).toEqual([
      { checkpointId: "ws-cp", repo: "workspace", path: "teamwork.md" },
    ]);
  });
});

describe("parseWorkspaceCheckpoints", () => {
  it("reads the flat single-checkpoint shape", () => {
    expect(parseWorkspaceCheckpoints({ checkpoint_id: "c1", checkpoint_repo: "team" })).toEqual([
      { id: "c1", repo: "team" },
    ]);
  });

  it("treats a response with no repo as the workspace repo", () => {
    expect(parseWorkspaceCheckpoints({ checkpoint_id: "c1" })).toEqual([
      { id: "c1", repo: "workspace" },
    ]);
  });

  it("reads the two-checkpoint list", () => {
    expect(
      parseWorkspaceCheckpoints({
        checkpoint_id: null,
        checkpoints: [
          { checkpoint_id: "w", checkpoint_repo: "workspace" },
          { checkpoint_id: "t", checkpoint_repo: "team" },
        ],
      }),
    ).toEqual([
      { id: "w", repo: "workspace" },
      { id: "t", repo: "team" },
    ]);
  });

  it("is empty when none was recorded", () => {
    expect(parseWorkspaceCheckpoints({ checkpoint_id: null })).toEqual([]);
  });
});

describe("notifyWithWorkspaceUndo", () => {
  beforeEach(() => {
    for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
    restore = vi.spyOn(configCoordinator, "restore");
    restore.mockReset();
  });

  it("restores a team path from the team repo, relative to team/", async () => {
    restore.mockResolvedValue({ checkpoint_id: "x", restored_paths: [] });
    notifyWithWorkspaceUndo(null, "Deleted a.md.", "team/wiki/a.md", [
      { id: "team-cp", repo: "team" },
    ]);
    [...toast.toasts.values()].at(-1)?.action?.onClick();
    await waitFor(() => {
      expect(restore).toHaveBeenCalledTimes(1);
    });
    expect(restore).toHaveBeenCalledWith(null, "team-cp", "team", "wiki/a.md");
  });

  it("restores an agent's own path through the agent the action ran on", async () => {
    restore.mockResolvedValue({ checkpoint_id: "x", restored_paths: [] });
    notifyWithWorkspaceUndo("atlas", "Deleted a.md.", "notes/a.md", [
      { id: "ws-cp", repo: "workspace" },
    ]);
    [...toast.toasts.values()].at(-1)?.action?.onClick();
    await waitFor(() => {
      expect(restore).toHaveBeenCalledTimes(1);
    });
    expect(restore).toHaveBeenCalledWith("atlas", "ws-cp", "workspace", "notes/a.md");
  });

  it("offers no Undo when no checkpoint applies to the path", () => {
    notifyWithWorkspaceUndo(null, "Deleted a.md.", "team/a.md", []);
    expect([...toast.toasts.values()].at(-1)?.action).toBeUndefined();
  });
});
