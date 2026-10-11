import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionsStore } from "./sessions.svelte";
import { notifications } from "./notifications.svelte";
import type { SessionSummary } from "./types";
import { waitFor } from "../test/wait";

function summary(
  runId: string,
  address: string,
  overrides: Partial<SessionSummary> = {},
): SessionSummary {
  return {
    address,
    run_id: runId,
    category: "spawned",
    source_label: "agent:researcher",
    state: "running",
    spawner: null,
    depth: 1,
    purpose: "",
    started_at: "2026-09-23T12:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
    ...overrides,
  };
}

function store(agent: string | null = "scout"): SessionsStore {
  return new SessionsStore({ agent, pushToMain: () => {} });
}

function respond(status: number, body: unknown): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(() =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("finding the run to show for a session address", () => {
  it("uses the run id the caller has, without looking anything up", async () => {
    const fetchMock = respond(200, { live: [], completed: [], next_cursor: null });
    await expect(store().resolveRun("spawned-x-1", "run-9")).resolves.toBe("run-9");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("uses the newest run it already knows at the address, live first", async () => {
    const fetchMock = respond(200, { live: [], completed: [], next_cursor: null });
    const s = store();
    s.handleFrame({ type: "session_started", session: summary("run-live", "spawned-x-1") });
    await expect(s.resolveRun("spawned-x-1", null)).resolves.toBe("run-live");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("asks the agent's own sessions route when the address isn't loaded", async () => {
    const fetchMock = respond(200, {
      live: [],
      completed: [summary("run-old", "spawned-y-1", { state: "completed" })],
      next_cursor: null,
    });
    await expect(store("atlas").resolveRun("spawned-y-1", null)).resolves.toBe("run-old");
    const [url] = fetchMock.mock.calls[0] as unknown as [string];
    expect(url).toBe("/api/agents/atlas/sessions?limit=1&address=spawned-y-1");
  });

  it("tells the user, and gives no run, when the address has none", async () => {
    respond(200, { live: [], completed: [], next_cursor: null });
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    await expect(store().resolveRun("spawned-z-1", null)).resolves.toBeNull();
    expect(surface).toHaveBeenCalledWith("error", "There's no record of the session spawned-z-1.");
  });

  it("tells the user when the lookup fails", async () => {
    respond(500, { error: "boom" });
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    await expect(store().resolveRun("spawned-z-1", null)).resolves.toBeNull();
    expect(surface).toHaveBeenCalledTimes(1);
    expect(surface.mock.calls[0]?.[0]).toBe("error");
  });
});

describe("the finished list", () => {
  it("loads every kind with the listing, and a kind's own first page when it is chosen", async () => {
    const fetchMock = respond(200, {
      live: [summary("run-live", "spawned-x-1")],
      completed: [summary("run-done", "scheduled-y-1", { category: "scheduled" })],
      next_cursor: "run-done",
    });
    const s = store("atlas");
    await s.refresh();
    expect(s.live.map((run) => run.run_id)).toEqual(["run-live"]);
    expect(s.finished.all.runs.map((run) => run.run_id)).toEqual(["run-done"]);
    expect(s.finished.all.nextCursor).toBe("run-done");
    expect(s.finished.scheduled.loaded).toBe(false);

    s.showFinished("scheduled");
    await waitFor(() => {
      expect(s.finished.scheduled.loaded).toBe(true);
    });
    expect(s.finishedKind).toBe("scheduled");
    const urls = fetchMock.mock.calls.map((call) => (call as unknown as [string])[0]);
    expect(urls).toContain("/api/agents/atlas/sessions?category=scheduled&limit=25");
  });

  it("puts a run that just finished at the top, with how it ended", () => {
    const s = store();
    s.handleFrame({ type: "session_started", session: summary("run-1", "spawned-x-1") });
    s.handleFrame({
      type: "session_completed",
      address: "spawned-x-1",
      run_id: "run-1",
      status: "failed",
      error: "the site timed out",
      error_details: null,
      episode_id: null,
    });
    expect(s.live).toEqual([]);
    const [finished] = s.finished.all.runs;
    expect(finished?.run_id).toBe("run-1");
    expect(finished?.state).toBe("completed");
    expect(finished?.outcome).toBe("failed");
    expect(finished?.error).toBe("the site timed out");
    // A kind not loaded yet takes it with its first page.
    expect(s.finished.spawned.runs).toEqual([]);
  });

  it("says when a kind's finished runs couldn't load", async () => {
    respond(500, { error: "boom" });
    const s = store();
    await s.finished.artifact.loadFirst();
    expect(s.finished.artifact.error).toMatch(/^Couldn't load the finished runs\./);
    expect(s.finished.artifact.loaded).toBe(false);
  });
});

describe("stopping from the list", () => {
  it("shows the stop until the run finishes", async () => {
    respond(202, { address: "spawned-x-1" });
    const s = store();
    s.handleFrame({ type: "session_started", session: summary("run-1", "spawned-x-1") });
    await s.stop("spawned-x-1");
    expect(s.stopping.has("spawned-x-1")).toBe(true);
    s.handleFrame({
      type: "session_completed",
      address: "spawned-x-1",
      run_id: "run-1",
      status: "cancelled",
      error: null,
      error_details: null,
      episode_id: null,
    });
    expect(s.stopping.has("spawned-x-1")).toBe(false);
  });

  it("tells the user when the stop failed, and offers it again", async () => {
    respond(404, { error: "not running", code: "not_live" });
    const surface = vi.spyOn(notifications, "surface").mockImplementation(() => {});
    const s = store();
    await s.stop("spawned-x-1");
    expect(s.stopping.has("spawned-x-1")).toBe(false);
    expect(surface).toHaveBeenCalledWith(
      "error",
      "Couldn't stop spawned-x-1. It had already finished.",
    );
  });

  it("exposes the agent it belongs to", () => {
    expect(store("atlas").agent).toBe("atlas");
    expect(store(null).agent).toBeNull();
  });
});
