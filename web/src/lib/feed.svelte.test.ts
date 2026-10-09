import { describe, expect, it } from "vitest";
import { FeedStore } from "./feed.svelte";
import type { RecentHistorySegment, RecentMessage, ServerMessage } from "./types";

function historyMsg(
  role: RecentMessage["role"],
  content: string,
  opts: { turnId?: string } = {},
): RecentMessage {
  return {
    role,
    content,
    timestamp: "2026-01-01T10:00",
    visibility: "user",
    turn_id: opts.turnId,
  };
}

function segment(
  messages: RecentMessage[],
  nextCursor: string | null = null,
): RecentHistorySegment {
  return { kind: "recent", messages, next_cursor: nextCursor };
}

/** Load a store with two settled messages unrelated to the turn under test. */
function storeWithSettledHistory(): FeedStore {
  const store = new FeedStore();
  store.loadHistory(segment([historyMsg("user", "unrelated"), historyMsg("assistant", "ack")]));
  return store;
}

/** Start a live turn (user message pushed, `turn_started` received). */
function startLiveTurn(store: FeedStore, content: string, turnId: string): void {
  store.pushUserMessage(content);
  store.handleMessage({ type: "turn_started", reply_to: turnId });
}

describe("FeedStore reconcileRecent end-of-turn decision", () => {
  it("does not end an in-flight turn just because identical text appears in history", () => {
    const store = storeWithSettledHistory();
    startLiveTurn(store, "hello", "t2");

    // Reconnect: history now has an unrelated message with identical text,
    // recorded under a different turn — the live turn hasn't finished.
    const fresh = segment([
      historyMsg("user", "unrelated"),
      historyMsg("assistant", "ack"),
      historyMsg("user", "hello", { turnId: "t-other" }),
    ]);
    expect(store.reconcileRecent(fresh)).toBe(true);

    expect(store.isProcessing).toBe(true);
    expect(store.activeTurnId).toBe("t2");
  });

  it("ends the live turn once history records it under its turn id", () => {
    const store = storeWithSettledHistory();
    startLiveTurn(store, "hello", "t2");

    const fresh = segment([
      historyMsg("user", "unrelated"),
      historyMsg("assistant", "ack"),
      historyMsg("user", "hello", { turnId: "t2" }),
      historyMsg("assistant", "hi there", { turnId: "t2" }),
    ]);
    expect(store.reconcileRecent(fresh)).toBe(true);

    expect(store.isProcessing).toBe(false);
    expect(store.activeTurnId).toBeNull();
    expect(store.feed.at(-1)).toMatchObject({ kind: "assistant", content: "hi there" });
  });

  it("leaves an idle feed unaffected when no turn is in flight", () => {
    const store = storeWithSettledHistory();

    const fresh = segment([
      historyMsg("user", "unrelated"),
      historyMsg("assistant", "ack"),
      historyMsg("user", "another message"),
    ]);
    expect(store.reconcileRecent(fresh)).toBe(true);

    expect(store.isProcessing).toBe(false);
    expect(store.activeTurnId).toBeNull();
    expect(store.feed.at(-1)).toMatchObject({ kind: "user", content: "another message" });
  });
});

describe("FeedStore reloadHistory end-of-turn decision", () => {
  it("does not end an in-flight turn just because identical text appears in history", () => {
    const store = storeWithSettledHistory();
    startLiveTurn(store, "hello", "t2");

    const fresh = segment([
      historyMsg("user", "unrelated"),
      historyMsg("assistant", "ack"),
      historyMsg("user", "hello", { turnId: "t-other" }),
    ]);
    store.reloadHistory(fresh);

    expect(store.isProcessing).toBe(true);
    expect(store.activeTurnId).toBe("t2");
    expect(store.feed.at(-1)).toMatchObject({ kind: "user", content: "hello" });
  });

  it("ends the live turn once history records it under its turn id", () => {
    const store = storeWithSettledHistory();
    startLiveTurn(store, "hello", "t2");

    const fresh = segment([
      historyMsg("user", "unrelated"),
      historyMsg("assistant", "ack"),
      historyMsg("user", "hello", { turnId: "t2" }),
      historyMsg("assistant", "hi there", { turnId: "t2" }),
    ]);
    store.reloadHistory(fresh);

    expect(store.isProcessing).toBe(false);
    expect(store.activeTurnId).toBeNull();
    expect(store.feed.at(-1)).toMatchObject({ kind: "assistant", content: "hi there" });
  });

  it("leaves an idle feed unaffected when no turn is in flight", () => {
    const store = storeWithSettledHistory();

    const fresh = segment([
      historyMsg("user", "unrelated"),
      historyMsg("assistant", "ack"),
      historyMsg("user", "another message"),
    ]);
    store.reloadHistory(fresh);

    expect(store.isProcessing).toBe(false);
    expect(store.activeTurnId).toBeNull();
    expect(store.feed.at(-1)).toMatchObject({ kind: "user", content: "another message" });
  });
});

describe("FeedStore with a turn id history already holds", () => {
  // Every page load once counted its message ids from web-1 again, and the
  // agent kept them as turn ids, so older history holds turns under ids a
  // new turn can still reuse.
  const earlier = [
    historyMsg("user", "Earlier question", { turnId: "web-1" }),
    historyMsg("assistant", "Earlier answer", { turnId: "web-1" }),
  ];

  function storeWithEarlierTurn(): FeedStore {
    const store = new FeedStore();
    store.loadHistory(segment(earlier));
    store.pushUserMessage("New question", undefined, "web-1");
    store.handleMessage({ type: "turn_started", reply_to: "web-1" });
    return store;
  }

  for (const how of ["reconcileRecent", "reloadHistory"] as const) {
    it(`keeps the new turn running while history holds only the earlier one (${how})`, () => {
      const store = storeWithEarlierTurn();
      const change = segment(earlier);
      if (how === "reconcileRecent") expect(store.reconcileRecent(change)).toBe(true);
      else store.reloadHistory(change);

      expect(store.isProcessing).toBe(true);
      expect(store.activeTurnId).toBe("web-1");
      expect(store.feed.at(-1)).toMatchObject({ kind: "user", content: "New question" });
    });

    it(`settles the new turn once history holds more under the id (${how})`, () => {
      const store = storeWithEarlierTurn();
      const change = segment([
        ...earlier,
        historyMsg("user", "New question", { turnId: "web-1" }),
        historyMsg("assistant", "New answer", { turnId: "web-1" }),
      ]);
      if (how === "reconcileRecent") expect(store.reconcileRecent(change)).toBe(true);
      else store.reloadHistory(change);

      expect(store.isProcessing).toBe(false);
      expect(store.activeTurnId).toBeNull();
      expect(store.feed.at(-1)).toMatchObject({ kind: "assistant", content: "New answer" });
    });
  }
});

describe("FeedStore flagging messages that reach a running turn", () => {
  it("flags a user message sent while a turn runs, and not one that starts a turn", () => {
    const store = storeWithSettledHistory();
    store.pushUserMessage("Draft the post", undefined, "web-a");
    store.handleMessage({ type: "turn_started", reply_to: "web-a" });
    store.pushUserMessage("Keep it short", undefined, "web-b");

    const users = store.feed.filter((item) => item.kind === "user").slice(-2);
    expect(users[0]).not.toHaveProperty("midTurn");
    expect(users[1]).toMatchObject({ content: "Keep it short", turnId: "web-a", midTurn: true });
  });

  it("flags a session's message that arrives mid-turn", () => {
    const store = storeWithSettledHistory();
    store.pushAgentMessage("spawned-1", "run-1", "Done early", null);
    store.pushUserMessage("Go", undefined, "web-a");
    store.handleMessage({ type: "turn_started", reply_to: "web-a" });
    store.pushAgentMessage("spawned-1", "run-1", "Result", null);

    const messages = store.feed.filter((item) => item.kind === "agent-message");
    expect(messages[0]).not.toHaveProperty("midTurn");
    expect(messages[1]).toMatchObject({ content: "Result", turnId: "web-a", midTurn: true });
  });
});

describe("FeedStore catching up on a turn whose frames are still arriving", () => {
  // History and the socket are separate connections: after a reconnect the
  // catch-up fetch can bring back a turn that has finished on the agent while
  // this page is still receiving its frames.
  const frames: ServerMessage[] = [
    { type: "turn_started", reply_to: "web-1" },
    { type: "broadcast_response", content: "Looking first." },
    { type: "tool_call", id: "c1", name: "memory_search", arguments: {}, server: null },
    {
      type: "tool_result",
      tool_call_id: "c1",
      name: "memory_search",
      output: "found",
      is_error: false,
    },
    { type: "response", reply_to: "web-1", content: "Here is what I found." },
    { type: "turn_ended", reply_to: "web-1" },
  ];

  const recorded = segment([
    historyMsg("user", "unrelated"),
    historyMsg("assistant", "ack"),
    historyMsg("user", "Are you there?", { turnId: "web-1" }),
    {
      ...historyMsg("assistant", "Looking first.", { turnId: "web-1" }),
      tool_calls: [{ id: "c1", name: "memory_search", arguments: {}, server: null }],
    },
    { ...historyMsg("tool", "found", { turnId: "web-1" }), tool_call_id: "c1" },
    historyMsg("assistant", "Here is what I found.", { turnId: "web-1" }),
  ]);

  function shown(store: FeedStore, content: string): number {
    return store.feed.filter(
      (item) => (item.kind === "user" || item.kind === "assistant") && item.content === content,
    ).length;
  }

  for (let arrived = 0; arrived <= frames.length; arrived++) {
    const after = arrived === 0 ? "before any frame" : `after ${frames[arrived - 1]?.type ?? ""}`;
    for (const how of ["reconcileRecent", "reloadHistory"] as const) {
      it(`shows the turn once when history arrives ${after} (${how})`, () => {
        const store = storeWithSettledHistory();
        store.pushUserMessage("Are you there?", undefined, "web-1");
        for (const frame of frames.slice(0, arrived)) store.handleMessage(frame);

        if (how === "reconcileRecent") expect(store.reconcileRecent(recorded)).toBe(true);
        else store.reloadHistory(recorded);
        for (const frame of frames.slice(arrived)) store.handleMessage(frame);

        expect(shown(store, "Are you there?")).toBe(1);
        expect(shown(store, "Looking first.")).toBe(1);
        expect(shown(store, "Here is what I found.")).toBe(1);
        expect(store.isProcessing).toBe(false);
        expect(store.activeTurnId).toBeNull();
      });
    }
  }

  it("keeps a turn that ended live when history read before it was recorded arrives", () => {
    const store = storeWithSettledHistory();
    store.pushUserMessage("Are you there?", undefined, "web-1");
    for (const frame of frames) store.handleMessage(frame);

    const behind = segment([historyMsg("user", "unrelated"), historyMsg("assistant", "ack")]);
    expect(store.reconcileRecent(behind)).toBe(true);

    expect(shown(store, "Are you there?")).toBe(1);
    expect(shown(store, "Here is what I found.")).toBe(1);
  });

  it("lets the next turn's frames through once the settled turn has ended", () => {
    const store = storeWithSettledHistory();
    store.pushUserMessage("Are you there?", undefined, "web-1");
    store.handleMessage({ type: "turn_started", reply_to: "web-1" });
    expect(store.reconcileRecent(recorded)).toBe(true);
    for (const frame of frames.slice(1)) store.handleMessage(frame);

    store.pushUserMessage("And now?", undefined, "web-2");
    store.handleMessage({ type: "turn_started", reply_to: "web-2" });
    store.handleMessage({ type: "broadcast_response", content: "Still here." });
    store.handleMessage({ type: "turn_ended", reply_to: "web-2" });

    expect(shown(store, "Still here.")).toBe(1);
  });

  it("lets a turn joined after another reconnect through, though the settled one never ended", () => {
    const store = storeWithSettledHistory();
    store.pushUserMessage("Are you there?", undefined, "web-1");
    store.handleMessage({ type: "turn_started", reply_to: "web-1" });
    expect(store.reconcileRecent(recorded)).toBe(true);

    store.markReconnectGap();
    store.handleMessage({ type: "broadcast_response", content: "A later turn." });

    expect(shown(store, "A later turn.")).toBe(1);
  });
});
