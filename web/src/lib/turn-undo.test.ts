import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as ApiModule from "./api";
import { turnChangedWorkspace, undoTurn } from "./turn-undo";
import type { CheckpointPage } from "./types";

const { fetchCheckpoints, undoCheckpoint } = vi.hoisted(() => ({
  fetchCheckpoints: vi.fn(),
  undoCheckpoint: vi.fn(),
}));
vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof ApiModule>()),
  fetchCheckpoints,
  undoCheckpoint,
}));

function page(items: CheckpointPage["items"]): CheckpointPage {
  return { items, next_cursor: null };
}

describe("turnChangedWorkspace", () => {
  beforeEach(() => {
    fetchCheckpoints.mockReset();
    undoCheckpoint.mockReset();
  });

  it("is true when the turn's turn-end checkpoint changed paths", async () => {
    fetchCheckpoints.mockResolvedValue(
      page([
        {
          id: "end1",
          timestamp: "2026-01-01T00:00:00Z",
          address: "main",
          run_id: null,
          turn_id: "turn-1",
          trigger: "turn_end",
          summary: "wrote notes.md",
          changed_path_count: 2,
        },
        {
          id: "start1",
          timestamp: "2026-01-01T00:00:00Z",
          address: "main",
          run_id: null,
          turn_id: "turn-1",
          trigger: "turn_start",
          summary: "outside edit",
          changed_path_count: 0,
        },
      ]),
    );

    await expect(turnChangedWorkspace("turn-1")).resolves.toBe(true);
    expect(fetchCheckpoints).toHaveBeenCalledWith({
      repo: "workspace",
      turnId: "turn-1",
      limit: 10,
    });
  });

  it("is false when the turn-end checkpoint changed nothing", async () => {
    fetchCheckpoints.mockResolvedValue(
      page([
        {
          id: "end1",
          timestamp: "2026-01-01T00:00:00Z",
          address: "main",
          run_id: null,
          turn_id: "turn-1",
          trigger: "turn_end",
          summary: "turn completed",
          changed_path_count: 0,
        },
      ]),
    );

    await expect(turnChangedWorkspace("turn-1")).resolves.toBe(false);
  });

  it("is false when the turn has no recorded checkpoints", async () => {
    fetchCheckpoints.mockResolvedValue(page([]));
    await expect(turnChangedWorkspace("turn-1")).resolves.toBe(false);
  });
});

describe("undoTurn", () => {
  beforeEach(() => {
    fetchCheckpoints.mockReset();
    undoCheckpoint.mockReset();
  });

  it("undoes the turn's turn-end checkpoint", async () => {
    fetchCheckpoints.mockResolvedValue(
      page([
        {
          id: "end1",
          timestamp: "2026-01-01T00:00:00Z",
          address: "main",
          run_id: null,
          turn_id: "turn-1",
          trigger: "turn_end",
          summary: "wrote notes.md",
          changed_path_count: 1,
        },
      ]),
    );
    undoCheckpoint.mockResolvedValue({
      checkpoint_id: "undo1",
      reverted_paths: ["notes.md"],
      skipped_paths: [],
    });

    const outcome = await undoTurn("turn-1");

    expect(undoCheckpoint).toHaveBeenCalledWith("end1", "workspace");
    expect(outcome?.reverted_paths).toEqual(["notes.md"]);
  });

  it("returns null when there is no turn-end checkpoint to undo", async () => {
    fetchCheckpoints.mockResolvedValue(page([]));
    await expect(undoTurn("turn-1")).resolves.toBeNull();
    expect(undoCheckpoint).not.toHaveBeenCalled();
  });
});
