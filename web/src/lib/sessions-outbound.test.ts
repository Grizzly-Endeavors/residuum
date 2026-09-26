import { afterEach, describe, expect, it, vi } from "vitest";
import { SessionsStore } from "./sessions.svelte";
import type { OutboundA2aTaskSummary } from "./types";

function task(overrides: Partial<OutboundA2aTaskSummary> = {}): OutboundA2aTaskSummary {
  return {
    task_id: "t1",
    agent: "laptop",
    sender_address: "main",
    state: "working",
    status_text: null,
    open: true,
    started_at: "2026-09-26T14:00:00.000Z",
    unreachable_since: null,
    ...overrides,
  };
}

function store(): SessionsStore {
  return new SessionsStore({ send: () => {}, pushToMain: () => {} });
}

function respond(status: number, body: unknown): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": "application/json" },
        }),
      ),
    ),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SessionsStore outbound A2A tasks", () => {
  it("adds, updates, and drops tasks from their frames", () => {
    const s = store();
    s.handleFrame({ type: "session_outbound_a2a_task", task: task() });
    s.handleFrame({
      type: "session_outbound_a2a_task",
      task: task({ task_id: "t2", started_at: "2026-09-26T15:00:00.000Z" }),
    });
    expect(s.outbound.map((t) => t.task_id)).toEqual(["t2", "t1"]);

    s.handleFrame({
      type: "session_outbound_a2a_task",
      task: task({ status_text: "halfway" }),
    });
    expect(s.outbound.find((t) => t.task_id === "t1")?.status_text).toBe("halfway");

    s.handleFrame({
      type: "session_outbound_a2a_task",
      task: task({ state: "completed", open: false }),
    });
    expect(s.outbound.map((t) => t.task_id)).toEqual(["t2"]);
  });

  it("offers stop-watching when the agent can't be reached to cancel", async () => {
    const s = store();
    s.handleFrame({ type: "session_outbound_a2a_task", task: task() });
    respond(502, {
      error: "Couldn't reach laptop to cancel the task. You can stop watching it instead.",
      code: "unreachable",
    });

    await s.stopOutbound("t1");

    expect(s.outboundUnreachable.get("t1")).toContain("laptop");
    expect(s.outboundStopping.has("t1")).toBe(false);
    expect(s.outbound).toHaveLength(1);

    respond(200, task({ state: "canceled", open: false }));
    await s.stopWatchingOutbound("t1");

    expect(s.outbound).toHaveLength(0);
    expect(s.outboundUnreachable.has("t1")).toBe(false);
  });

  it("removes a task the stop cancelled", async () => {
    const s = store();
    s.handleFrame({ type: "session_outbound_a2a_task", task: task() });
    respond(200, task({ state: "canceled", open: false }));

    await s.stopOutbound("t1");

    expect(s.outbound).toHaveLength(0);
    expect(s.outboundUnreachable.has("t1")).toBe(false);
  });

  it("reports a failed listing without touching the sessions list", async () => {
    const s = store();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new TypeError("Failed to fetch"))),
    );

    await s.refreshOutbound();

    expect(s.outboundError).toContain("Couldn't load the tasks sent to other agents.");
    expect(s.listError).toBeNull();
  });
});
