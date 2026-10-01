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
  return new SessionsStore({ agent: "scout", pushToMain: () => {} });
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

  it("takes a task as a stop answered it, ahead of its frame", () => {
    const s = store();
    s.handleFrame({ type: "session_outbound_a2a_task", task: task() });
    s.applyOutbound(task({ state: "canceled", open: false }));
    expect(s.outbound).toHaveLength(0);
  });

  it("loads the open tasks", async () => {
    respond(200, [task({ task_id: "t9" })]);
    const s = store();
    await s.refreshOutbound();
    expect(s.outbound.map((t) => t.task_id)).toEqual(["t9"]);
    expect(s.outboundError).toBeNull();
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
