import { describe, expect, it } from "vitest";
import { SessionView } from "./sessions.svelte";

/** A view with `load()` bypassed, so `applyFrame` applies frames directly
 * instead of buffering them for a transcript fetch this test never makes. */
function readyView(): SessionView {
  const view = new SessionView("run-1", null, "scout");
  view.loading = false;
  return view;
}

describe("SessionView turn usage", () => {
  it("sets a live turn clock and resets token and tool-call progress on session_turn_started", () => {
    const view = readyView();
    const before = Date.now();
    view.applyFrame({
      type: "session_turn_started",
      address: "spawned-x-0001",
      run_id: "run-1",
      turn_id: "t1",
    });
    expect(view.turnStartedAt).not.toBeNull();
    expect(view.turnStartedAt).toBeGreaterThanOrEqual(before);
    expect(view.turnOutputTokens).toBe(0);
    expect(view.turnHasUsage).toBe(false);
    expect(view.turnToolCalls).toBe(0);
  });

  it("updates tool-call progress from session_turn_usage", () => {
    const view = readyView();
    view.applyFrame({
      type: "session_turn_started",
      address: "spawned-x-0001",
      run_id: "run-1",
      turn_id: "t1",
    });
    view.applyFrame({
      type: "session_turn_usage",
      address: "spawned-x-0001",
      run_id: "run-1",
      output_tokens: 30,
      has_usage: true,
      tool_calls: 5,
      session_totals: null,
    });
    expect(view.turnOutputTokens).toBe(30);
    expect(view.turnToolCalls).toBe(5);
  });

  it("a turn with zero tool calls reports a zero count, not nothing", () => {
    const view = readyView();
    view.applyFrame({
      type: "session_turn_started",
      address: "spawned-x-0001",
      run_id: "run-1",
      turn_id: "t1",
    });
    view.applyFrame({
      type: "session_turn_usage",
      address: "spawned-x-0001",
      run_id: "run-1",
      output_tokens: 12,
      has_usage: true,
      tool_calls: 0,
      session_totals: null,
    });
    expect(view.turnToolCalls).toBe(0);
  });

  it("folds the updated session totals' tool_calls into the summary", () => {
    const view = readyView();
    view.summary = {
      address: "spawned-x-0001",
      run_id: "run-1",
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
    };
    view.applyFrame({
      type: "session_turn_usage",
      address: "spawned-x-0001",
      run_id: "run-1",
      output_tokens: 8,
      has_usage: true,
      tool_calls: 2,
      session_totals: { input_tokens: 100, output_tokens: 8, context_tokens: 100, tool_calls: 2 },
    });
    expect(view.summary.usage.tool_calls).toBe(2);
  });
});
