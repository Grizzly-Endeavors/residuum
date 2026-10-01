import { describe, expect, it } from "vitest";
import { FeedStore } from "./feed.svelte";
import type { SessionUsageTotals } from "./types";

function totals(
  input: number,
  output: number,
  context: number | null = null,
  toolCalls = 0,
): SessionUsageTotals {
  return {
    input_tokens: input,
    output_tokens: output,
    context_tokens: context,
    tool_calls: toolCalls,
  };
}

describe("FeedStore turn usage", () => {
  it("sets a live turn clock and resets token and tool-call progress on turn_started", () => {
    const store = new FeedStore();
    const before = Date.now();
    store.handleMessage({ type: "turn_started", reply_to: "t1" });
    expect(store.observed.get("t1")?.startedAt).toBeGreaterThanOrEqual(before);
    expect(store.turnOutputTokens).toBe(0);
    expect(store.turnHasUsage).toBe(false);
    expect(store.turnToolCalls).toBe(0);
  });

  it("updates turn progress from turn_usage without a session total", () => {
    const store = new FeedStore();
    store.handleMessage({ type: "turn_started", reply_to: "t1" });
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 42,
      has_usage: true,
      tool_calls: 3,
      session_totals: null,
    });
    expect(store.turnOutputTokens).toBe(42);
    expect(store.turnHasUsage).toBe(true);
    expect(store.turnToolCalls).toBe(3);
    expect(store.sessionUsage).toBeNull();
  });

  it("updates cumulative session totals from turn_usage when present", () => {
    const store = new FeedStore();
    store.handleMessage({ type: "turn_started", reply_to: "t1" });
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 20,
      has_usage: true,
      tool_calls: 2,
      session_totals: totals(100, 20, 100, 2),
    });
    expect(store.sessionUsage).toEqual(totals(100, 20, 100, 2));
  });

  it("a provider with no usage still ticks the indicator without a token count", () => {
    const store = new FeedStore();
    store.handleMessage({ type: "turn_started", reply_to: "t1" });
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 0,
      has_usage: false,
      tool_calls: 0,
      session_totals: null,
    });
    expect(store.turnHasUsage).toBe(false);
    expect(store.turnOutputTokens).toBe(0);
    expect(store.turnToolCalls).toBe(0);
  });

  it("keeps counting tool calls across a batch that carried zero of them", () => {
    const store = new FeedStore();
    store.handleMessage({ type: "turn_started", reply_to: "t1" });
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 5,
      has_usage: true,
      tool_calls: 4,
      session_totals: null,
    });
    // A later usage event still carries the turn's running total (never
    // reset mid-turn), the same way output_tokens does.
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 12,
      has_usage: true,
      tool_calls: 4,
      session_totals: null,
    });
    expect(store.turnToolCalls).toBe(4);
  });

  it("stops the turn clock on turn_ended but keeps the session totals", () => {
    const store = new FeedStore();
    store.handleMessage({ type: "turn_started", reply_to: "t1" });
    store.setInitialUsage(totals(50, 10, 50));
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    expect(store.observed.get("t1")?.endedAt).not.toBeNull();
    expect(store.sessionUsage).toEqual(totals(50, 10, 50));
  });

  it("setInitialUsage seeds the footer before any turn has run", () => {
    const store = new FeedStore();
    expect(store.sessionUsage).toBeNull();
    store.setInitialUsage(totals(300, 60, 300));
    expect(store.sessionUsage).toEqual(totals(300, 60, 300));
  });

  it("tracks background post-turn activity and clears it on disconnect", () => {
    const store = new FeedStore();
    store.handleMessage({ type: "post_turn_activity", kind: "memory", active: true });
    store.handleMessage({ type: "post_turn_activity", kind: "subconscious", active: true });
    expect(store.memoryWorking).toBe(true);
    expect(store.subconsciousWorking).toBe(true);

    store.handleMessage({ type: "post_turn_activity", kind: "subconscious", active: false });
    expect(store.subconsciousWorking).toBe(false);
    expect(store.memoryWorking).toBe(true);

    store.clearPostTurnActivity();
    expect(store.memoryWorking).toBe(false);
  });
});
