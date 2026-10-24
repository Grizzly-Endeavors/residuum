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

function checkpoint(
  id: string,
  trigger: "turn_start" | "turn_end",
  changedPathCount: number,
): CheckpointPage["items"][number] {
  return {
    id,
    timestamp: "2026-01-01T00:00:00Z",
    address: "main",
    run_id: null,
    turn_id: "turn-1",
    trigger,
    summary: "",
    changed_path_count: changedPathCount,
  };
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

    await expect(turnChangedWorkspace("scout", "turn-1")).resolves.toBe(true);
    expect(fetchCheckpoints).toHaveBeenCalledWith("scout", {
      repo: "workspace",
      turnId: "turn-1",
      limit: 50,
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

    await expect(turnChangedWorkspace("scout", "turn-1")).resolves.toBe(false);
  });

  it("is true when any attempt of a retried turn changed paths", async () => {
    // The rerun changed nothing; the failed attempt before it did.
    fetchCheckpoints.mockResolvedValue(
      page([
        checkpoint("end-rerun", "turn_end", 0),
        checkpoint("start-rerun", "turn_start", 0),
        checkpoint("end-failed", "turn_end", 1),
        checkpoint("start-failed", "turn_start", 0),
      ]),
    );

    await expect(turnChangedWorkspace("scout", "turn-1")).resolves.toBe(true);
  });

  it("is false when the turn has no recorded checkpoints", async () => {
    fetchCheckpoints.mockResolvedValue(page([]));
    await expect(turnChangedWorkspace("scout", "turn-1")).resolves.toBe(false);
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

    const outcome = await undoTurn("scout", "turn-1");

    expect(undoCheckpoint).toHaveBeenCalledWith("scout", "end1", "workspace");
    expect(outcome?.reverted_paths).toEqual(["notes.md"]);
  });

  it("undoes every attempt of a retried turn, newest first, and merges the outcomes", async () => {
    // A retried turn keeps its id: the failed attempt and the rerun each
    // recorded a pair, and the rerun's pair comes back on a second page.
    fetchCheckpoints
      .mockResolvedValueOnce({
        items: [checkpoint("end-rerun", "turn_end", 2), checkpoint("start-rerun", "turn_start", 0)],
        next_cursor: "cursor-1",
      })
      .mockResolvedValueOnce(
        page([
          checkpoint("end-failed", "turn_end", 2),
          checkpoint("start-failed", "turn_start", 0),
        ]),
      );
    undoCheckpoint
      .mockResolvedValueOnce({
        checkpoint_id: "undo-rerun",
        reverted_paths: ["notes.md", "plan.md"],
        skipped_paths: [],
      })
      .mockResolvedValueOnce({
        checkpoint_id: "undo-failed",
        reverted_paths: ["notes.md"],
        skipped_paths: ["draft.md"],
      });

    const outcome = await undoTurn("scout", "turn-1");

    expect(fetchCheckpoints).toHaveBeenNthCalledWith(2, "scout", {
      repo: "workspace",
      turnId: "turn-1",
      before: "cursor-1",
      limit: 50,
    });
    expect(undoCheckpoint.mock.calls.map((call: unknown[]) => call[1])).toEqual([
      "end-rerun",
      "end-failed",
    ]);
    expect(outcome).toEqual({
      checkpoint_id: "undo-failed",
      reverted_paths: ["notes.md", "plan.md"],
      skipped_paths: ["draft.md"],
    });
  });

  it("returns null when there is no turn-end checkpoint to undo", async () => {
    fetchCheckpoints.mockResolvedValue(page([]));
    await expect(undoTurn("scout", "turn-1")).resolves.toBeNull();
    expect(undoCheckpoint).not.toHaveBeenCalled();
  });
});
