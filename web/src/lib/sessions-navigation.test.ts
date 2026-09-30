import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionsStore } from "./sessions.svelte";
import { notifications } from "./notifications.svelte";
import type { SessionSummary } from "./types";

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
  return new SessionsStore({ agent, send: () => {}, pushToMain: () => {} });
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

describe("a session that continues in a new run", () => {
  it("moves the open view to the new run, leaving the location to the view that shows it", () => {
    const s = store();
    s.showRun("run-1");
    const view = s.view;
    expect(view?.runId).toBe("run-1");
    if (view === null) return;
    view.loading = false;
    view.followAddress = "spawned-x-1";
    s.handleFrame({ type: "session_started", session: summary("run-2", "spawned-x-1") });
    expect(s.view?.runId).toBe("run-2");
  });

  it("doesn't touch a view of another session", () => {
    const s = store();
    s.showRun("run-1");
    if (s.view !== null) s.view.followAddress = "spawned-x-1";
    s.handleFrame({ type: "session_started", session: summary("run-3", "spawned-other-1") });
    expect(s.view?.runId).toBe("run-1");
  });

  it("exposes the agent it belongs to", () => {
    expect(store("atlas").agent).toBe("atlas");
    expect(store(null).agent).toBeNull();
  });
});
