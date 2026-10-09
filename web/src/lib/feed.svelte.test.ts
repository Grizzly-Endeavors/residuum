import { describe, expect, it } from "vitest";
import { groupTurns } from "../feed/turns";
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
  store.handleMessage({
    type: "turn_started",
    reply_to: turnId,
    origin: { endpoint: "ws", visibility: "user" },
  });
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

describe("FeedStore dividers", () => {
  it("names the day an episode ends on, and keeps its id apart", () => {
    const store = new FeedStore();
    store.prependEpisode({
      kind: "episode",
      episode_id: "ep-002",
      date: "2026-03-13",
      messages: [historyMsg("user", "Hi"), historyMsg("assistant", "Hello.")],
      next_cursor: null,
    });
    expect(store.feed[0]).toMatchObject({
      kind: "divider",
      variant: "episode",
      date: "2026-03-13",
      episode: "ep-002",
    });
  });

  it("puts a day divider where history crosses into another day", () => {
    const store = new FeedStore();
    store.loadHistory(
      segment([
        { ...historyMsg("user", "Late"), timestamp: "2026-03-13T23:50" },
        { ...historyMsg("assistant", "Still up?"), timestamp: "2026-03-13T23:51" },
        { ...historyMsg("user", "Morning"), timestamp: "2026-03-14T08:00" },
      ]),
    );
    const dividers = store.feed.filter((item) => item.kind === "divider");
    expect(dividers).toMatchObject([{ variant: "day", date: "2026-03-14" }]);
  });
});

describe("FeedStore when a turn fails", () => {
  const error = (replyTo: string | null, details: string | null = null): ServerMessage => ({
    type: "error",
    reply_to: replyTo,
    message: "The model provider didn't answer.",
    details,
  });

  function failingTurn(store: FeedStore, id = "web-a"): void {
    store.pushUserMessage("Tidy the wiki index", undefined, id);
    store.handleMessage({
      type: "turn_started",
      reply_to: id,
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "t1",
      call: 0,
      content: "Looking first.",
    });
  }

  it("leaves an account of it in the turn, with its cause and what to send again", () => {
    const store = new FeedStore();
    failingTurn(store);
    store.handleMessage(error("web-a", "provider returned 503"));
    store.handleMessage({ type: "turn_ended", reply_to: "web-a" });

    expect(store.isProcessing).toBe(false);
    expect(store.feed.at(-1)).toEqual({
      id: expect.any(Number) as number,
      kind: "turn-failure",
      turnId: "web-a",
      message: "The model provider didn't answer.",
      details: "provider returned 503",
      retry: { content: "Tidy the wiki index" },
    });
  });

  it("shows the user's message after a reload, with the failure's account gone", () => {
    const live = new FeedStore();
    live.pushUserMessage("Hello", undefined, "web-a");
    live.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    live.handleMessage(error("web-a"));
    live.handleMessage({ type: "turn_ended", reply_to: "web-a" });
    expect(live.feed.map((item) => item.kind)).toEqual(["user", "turn-failure"]);

    // The server recorded the message of the turn that failed, and nothing else.
    const reloaded = new FeedStore();
    reloaded.loadHistory(segment([historyMsg("user", "Hello", { turnId: "web-a" })]));
    expect(reloaded.feed.map((item) => item.kind)).toEqual(["user"]);
  });

  it("keeps the images the user sent for Try again", () => {
    const store = new FeedStore();
    const image = { media_type: "image/png", data: "AAAA" };
    store.pushUserMessage("What is this?", [image], "web-a");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(error("web-a"));

    expect(store.feed.at(-1)).toMatchObject({
      retry: { content: "What is this?", images: [image] },
    });
  });

  it("puts the failure inside its turn's block, so the turn ends on it", () => {
    const store = new FeedStore();
    failingTurn(store);
    store.handleMessage(error("web-a"));

    const entries = groupTurns(store.feed, store.activeTurnId);
    const block = entries.at(-1);
    if (block?.kind !== "turn") throw new Error("expected the turn's block");
    expect(
      block.parts.map((part) => (part.kind === "message" ? part.item.kind : part.kind)),
    ).toEqual(["assistant", "turn-failure"]);
  });

  it("shows a turn that failed before it did anything", () => {
    const store = new FeedStore();
    store.pushUserMessage("Hello", undefined, "web-a");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(error("web-a"));
    store.handleMessage({ type: "turn_ended", reply_to: "web-a" });

    const entries = groupTurns(store.feed, store.activeTurnId);
    expect(entries.map((entry) => entry.kind)).toEqual(["single", "turn"]);
    expect(store.feed.at(-1)).toMatchObject({ kind: "turn-failure" });
  });

  it("offers no retry for a message the page doesn't hold, or one that wasn't the user's own", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t9",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(error("t9"));
    expect(store.feed.at(-1)).not.toHaveProperty("retry");
  });

  it("keeps an error that names no turn to the toast", () => {
    const store = new FeedStore();
    failingTurn(store);
    const before = store.feed.length;
    store.handleMessage(error(null));
    expect(store.feed).toHaveLength(before);
    expect(store.isProcessing).toBe(false);
  });

  it("puts the failure in a turn the page joined partway", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "t5",
      call: 0,
      content: "Working on it.",
    });
    store.handleMessage(error("t5"));
    expect(store.activeTurnId).toBe("t5");
    expect(store.feed.every((item) => item.turnId === "t5")).toBe(true);
  });
});

describe("FeedStore announcements", () => {
  function named(): FeedStore {
    return new FeedStore(
      () => null,
      () => "atlas",
    );
  }

  it("says the agent is working when a turn starts", () => {
    const store = named();
    expect(store.announcement).toBeNull();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    expect(store.announcement?.text).toBe("atlas is working");
  });

  it("says the reply is complete, with the start of it, and not before", () => {
    const store = named();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "t1",
      call: 0,
      content: "Looking through the notes.",
    });
    expect(store.announcement?.text).toBe("atlas is working");

    store.handleMessage({
      type: "response",
      reply_to: "t1",
      endpoint: "ws",
      content: "**Done.** Fixed the port.",
    });
    expect(store.announcement?.text).toBe("atlas replied: Done. Fixed the port.");
  });

  it("says nothing for a reply with no words, or the tools it ran", () => {
    const store = named();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    const started = store.announcement;
    store.handleMessage({
      type: "tool_call",
      reply_to: "t1",
      call: 0,
      id: "c1",
      name: "exec",
      arguments: {},
      server: null,
    });
    store.handleMessage({ type: "response", reply_to: "t1", endpoint: "ws", content: "" });
    expect(store.announcement).toBe(started);
  });

  it("says the turn couldn't finish, for an error that names it", () => {
    const store = named();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({ type: "error", reply_to: "t1", message: "Nope.", details: null });
    expect(store.announcement?.text).toBe("atlas couldn't finish");
  });

  it("leaves an error that names no turn to its toast", () => {
    const store = named();
    store.handleMessage({ type: "error", reply_to: null, message: "Bad frame.", details: null });
    expect(store.announcement).toBeNull();
  });

  it("makes each announcement a new one, so the same words twice are both read", () => {
    const store = named();
    store.handleMessage({
      type: "turn_started",
      reply_to: "t1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    const first = store.announcement;
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    store.handleMessage({
      type: "turn_started",
      reply_to: "t2",
      origin: { endpoint: "ws", visibility: "user" },
    });
    expect(store.announcement?.text).toBe(first?.text);
    expect(store.announcement?.id).not.toBe(first?.id);
  });

  it("stays quiet about a turn history already showed", () => {
    const store = named();
    store.loadHistory(
      segment([
        historyMsg("user", "Are you there?", { turnId: "web-1" }),
        historyMsg("assistant", "Here.", { turnId: "web-1" }),
      ]),
    );
    store.pushUserMessage("Are you there?", undefined, "web-1");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    const before = store.announcement;
    expect(
      store.reconcileRecent(
        segment([
          historyMsg("user", "Are you there?", { turnId: "web-1" }),
          historyMsg("assistant", "Here.", { turnId: "web-1" }),
          historyMsg("user", "Are you there?", { turnId: "web-1" }),
          historyMsg("assistant", "Still here.", { turnId: "web-1" }),
        ]),
      ),
    ).toBe(true);
    store.handleMessage({
      type: "response",
      reply_to: "web-1",
      endpoint: "ws",
      content: "Still here.",
    });
    expect(store.announcement).toBe(before);
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
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-1",
      origin: { endpoint: "ws", visibility: "user" },
    });
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
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.pushUserMessage("Keep it short", undefined, "web-b");

    const users = store.feed.filter((item) => item.kind === "user").slice(-2);
    expect(users[0]).not.toHaveProperty("midTurn");
    expect(users[1]).toMatchObject({ content: "Keep it short", turnId: "web-a", midTurn: true });
  });

  it("flags a session's message that arrives mid-turn", () => {
    const store = storeWithSettledHistory();
    store.pushAgentMessage("spawned-1", "run-1", "Done early", null);
    store.pushUserMessage("Go", undefined, "web-a");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
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
    { type: "turn_started", reply_to: "web-1", origin: { endpoint: "ws", visibility: "user" } },
    { type: "broadcast_response", reply_to: "web-1", call: 0, content: "Looking first." },
    {
      type: "tool_call",
      reply_to: "web-1",
      call: 0,
      id: "c1",
      name: "memory_search",
      arguments: {},
      server: null,
    },
    {
      type: "tool_result",
      reply_to: "web-1",
      tool_call_id: "c1",
      name: "memory_search",
      output: "found",
      is_error: false,
    },
    { type: "response", reply_to: "web-1", endpoint: "ws", content: "Here is what I found." },
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
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    expect(store.reconcileRecent(recorded)).toBe(true);
    for (const frame of frames.slice(1)) store.handleMessage(frame);

    store.pushUserMessage("And now?", undefined, "web-2");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-2",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "web-1",
      call: 0,
      content: "Still here.",
    });
    store.handleMessage({ type: "turn_ended", reply_to: "web-2" });

    expect(shown(store, "Still here.")).toBe(1);
  });

  it("lets a turn joined after another reconnect through, though the settled one never ended", () => {
    const store = storeWithSettledHistory();
    store.pushUserMessage("Are you there?", undefined, "web-1");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    expect(store.reconcileRecent(recorded)).toBe(true);

    store.markReconnectGap();
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "web-1",
      call: 0,
      content: "A later turn.",
    });

    expect(shown(store, "A later turn.")).toBe(1);
  });
});
