import { describe, expect, it } from "vitest";
import { FeedStore } from "./feed.svelte";
import type { RecentHistorySegment, RecentMessage } from "./types";

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
