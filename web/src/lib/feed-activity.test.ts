import { describe, expect, it, vi } from "vitest";
import { groupTurns, type FeedTurn } from "../feed/turns";
import { FeedStore } from "./feed.svelte";
import type { ServerMessage, ToolCallState } from "./types";

// The feed store's record of each turn it watched, for the activity line:
// timing, failures, how the turn ended, and steps the page may have missed.

function toolCall(id: string, name = "read_file", server: string | null = null): ServerMessage {
  return {
    type: "tool_call",
    reply_to: "t1",
    call: 0,
    id,
    name,
    arguments: { path: "team/wiki/index.md" },
    server,
  };
}

function toolResult(id: string, isError = false): ServerMessage {
  return {
    type: "tool_result",
    reply_to: "t1",
    tool_call_id: id,
    name: "read_file",
    output: "ok",
    is_error: isError,
  };
}

function liveBlock(store: FeedStore): FeedTurn | undefined {
  return groupTurns(store.feed, store.activeTurnId).find(
    (entry): entry is FeedTurn => entry.kind === "turn" && entry.live,
  );
}

/** Every tool call in `block`, across its activity segments. */
function callsOf(block: FeedTurn | undefined): ToolCallState[] {
  return block?.parts.flatMap((part) => (part.kind === "activity" ? part.calls : [])) ?? [];
}

function blocks(store: FeedStore): FeedTurn[] {
  return groupTurns(store.feed, store.activeTurnId).filter(
    (entry): entry is FeedTurn => entry.kind === "turn",
  );
}

describe("a turn the page watched", () => {
  it("is timed from turn_started to turn_ended, and keeps a failed step's status", () => {
    const store = new FeedStore();
    store.pushUserMessage("check the wiki");
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(toolCall("c1", "read_file", "github"));
    store.handleMessage(toolResult("c1", true));
    expect(callsOf(liveBlock(store))).toMatchObject([{ status: "error", server: "github" }]);

    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    const record = store.observed.get("t1");
    expect(record?.ending).toBe("finished");
    expect(record?.endedAt).toBeGreaterThanOrEqual(record?.startedAt ?? Infinity);
    expect(callsOf(blocks(store)[0]).map((c) => c.status)).toEqual(["error"]);
  });

  it("times each step from its call to its result, or to the end of the turn", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(10_000);
      const store = new FeedStore();
      store.handleMessage({
        type: "turn_started",
        reply_to: "t1",
        origin: { endpoint: "ws", visibility: "user" },
      });
      store.handleMessage(toolCall("c1"));
      store.handleMessage(toolCall("c2"));
      vi.setSystemTime(12_500);
      store.handleMessage(toolResult("c1"));
      vi.setSystemTime(14_000);
      store.handleMessage({ type: "turn_ended", reply_to: "t1" });

      expect(callsOf(blocks(store)[0])).toMatchObject([
        { id: "c1", startedAt: 10_000, endedAt: 12_500 },
        { id: "c2", startedAt: 10_000, endedAt: 14_000 },
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("is stopped by the user: steps still running are marked stopped", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(toolCall("c1", "exec"));
    store.askStop();
    expect(store.observed.get("t1")?.stopAsked).toBe(true);

    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    expect(store.observed.get("t1")?.ending).toBe("stopped");
    expect(callsOf(blocks(store)[0]).map((c) => c.status)).toEqual(["stopped"]);
  });

  it("is cut off when the agent stops", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(toolCall("c1"));
    store.abandonLiveTurn();
    expect(store.observed.get("t1")?.ending).toBe("interrupted");
    expect(store.activeTurnId).toBeNull();
  });

  it("shows from turn_started, before any output", () => {
    const store = new FeedStore();
    store.pushUserMessage("hello");
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    expect(liveBlock(store)).toMatchObject({ key: "turn:t1", parts: [] });

    // The same block, by key, once output arrives.
    store.handleMessage(toolCall("c1"));
    expect(liveBlock(store)?.key).toBe("turn:t1");
  });

  it("notes where steps may be missing after the connection came back", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(toolCall("c1"));
    store.handleMessage(toolCall("c2"));
    store.markReconnectGap();
    expect(store.observed.get("t1")?.gaps).toEqual([2]);
  });

  it("is dropped once history renders the turn again, which has no timing", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    expect(store.observed.get("t1")).toBeDefined();
    store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    expect(store.observed.get("t1")).toBeUndefined();
  });
});

describe("a turn the page joined already running", () => {
  it("starts on its first frame, with a note that earlier steps aren't shown", () => {
    const store = new FeedStore(() => 5_000);
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "t1",
      call: 0,
      content: "Looking first.",
    });
    store.handleMessage(toolCall("c1"));

    const turnId = store.activeTurnId;
    expect(turnId).not.toBeNull();
    expect(store.isProcessing).toBe(true);
    expect(store.observed.get(turnId ?? "")).toMatchObject({ startedAt: 5_000, gaps: [0] });
    const block = liveBlock(store);
    expect(callsOf(block)).toHaveLength(1);
    expect(
      block?.parts.map((part) => (part.kind === "message" ? part.item.kind : part.kind)),
    ).toEqual(["assistant", "activity"]);
  });

  it("takes the turn's id from the first frame it sees, which every one of its frames carries", () => {
    const store = new FeedStore();
    store.handleMessage({ ...toolCall("c1"), reply_to: "t9" } as ServerMessage);
    expect(store.activeTurnId).toBe("t9");
    expect(store.feed.every((item) => item.turnId === "t9")).toBe(true);
    expect(store.observed.get("t9")?.gaps).toEqual([0]);

    store.handleMessage({ type: "response", reply_to: "t9", endpoint: "ws", content: "Done." });
    store.handleMessage({ type: "turn_ended", reply_to: "t9" });
    expect(store.observed.get("t9")?.ending).toBe("finished");
    expect(blocks(store)).toHaveLength(1);
  });

  it("starts with its own id when the first frame carries one", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t3",
      output_tokens: 0,
      has_usage: false,
      tool_calls: 0,
      session_totals: null,
    });
    expect(store.activeTurnId).toBe("t3");
  });

  it("never restarts a turn that already ended", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    store.handleMessage({
      type: "turn_usage",
      reply_to: "t1",
      output_tokens: 0,
      has_usage: false,
      tool_calls: 0,
      session_totals: null,
    });
    expect(store.activeTurnId).toBeNull();
  });
});
