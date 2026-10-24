import { afterEach, describe, expect, it, vi } from "vitest";
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

describe("FeedStore day dividers for live messages around local midnight", () => {
  /** A store whose history ends at `last`, a local timestamp, read with the clock set to `now`. */
  function storeEndingAt(last: string, now: string): FeedStore {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(now));
    const store = new FeedStore();
    store.loadHistory(segment([{ ...historyMsg("assistant", "Earlier."), timestamp: last }]));
    return store;
  }

  const dividers = (store: FeedStore): unknown[] =>
    store.feed.filter((item) => item.kind === "divider");

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllEnvs();
  });

  it("starts no new day while it is still the same day here, though it is the next day in UTC", () => {
    vi.stubEnv("TZ", "America/Los_Angeles");
    // 23:30 on the 9th in Los Angeles is 06:30 on the 10th in UTC.
    const store = storeEndingAt("2026-10-09T23:10:00", "2026-10-10T06:30:00Z");
    store.pushUserMessage("Still the same evening");
    expect(dividers(store)).toEqual([]);
  });

  it("starts the new day when midnight passes here", () => {
    vi.stubEnv("TZ", "America/Los_Angeles");
    // 00:10 on the 10th in Los Angeles is 07:10 on the 10th in UTC.
    const store = storeEndingAt("2026-10-09T23:40:00", "2026-10-10T07:10:00Z");
    store.pushUserMessage("Just after midnight");
    expect(dividers(store)).toMatchObject([{ variant: "day", date: "2026-10-10" }]);
  });

  it("starts the new day just after midnight east of UTC, where UTC is still on the old day", () => {
    vi.stubEnv("TZ", "Pacific/Auckland");
    // 00:30 on the 10th in Auckland is 11:30 on the 9th in UTC.
    const store = storeEndingAt("2026-10-09T23:50:00", "2026-10-09T11:30:00Z");
    store.pushUserMessage("Just after midnight");
    expect(dividers(store)).toMatchObject([{ variant: "day", date: "2026-10-10" }]);
  });

  it("holds for a message another channel brings, and for the one after it", () => {
    vi.stubEnv("TZ", "Pacific/Auckland");
    const store = storeEndingAt("2026-10-09T23:50:00", "2026-10-09T10:50:00Z");
    store.handleMessage({
      type: "user_message",
      id: "tg-1",
      turn_id: "tg-1",
      content: "Before midnight",
      endpoint: "telegram",
    });
    expect(dividers(store)).toEqual([]);

    vi.setSystemTime(new Date("2026-10-09T11:05:00Z"));
    store.handleMessage({
      type: "user_message",
      id: "tg-2",
      turn_id: "tg-2",
      content: "After midnight",
      endpoint: "telegram",
    });
    store.pushUserMessage("Later still");
    expect(dividers(store)).toMatchObject([{ variant: "day", date: "2026-10-10" }]);
  });
});

describe("FeedStore when a turn fails", () => {
  const error = (replyTo: string | null, details?: string): ServerMessage => ({
    type: "error",
    reply_to: replyTo,
    message: "The model provider didn't answer.",
    ...(details === undefined ? {} : { details }),
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

  it("leaves an account of it in the turn, with its cause and Try again", () => {
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
      retry: {},
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

  it("offers Try again for a message that carried files too: the agent still holds them", () => {
    const store = new FeedStore();
    const file = {
      item_id: "",
      filename: "notes.pdf",
      size: 10,
      content_type: "application/pdf",
      path: "",
    };
    store.pushUserMessage("What is this?", [file], "web-a");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(error("web-a"));

    expect(store.feed.at(-1)).toMatchObject({ kind: "turn-failure", retry: {} });
  });

  describe("and the user tries again", () => {
    const started = (id: string): ServerMessage => ({
      type: "turn_started",
      reply_to: id,
      origin: { endpoint: "ws", visibility: "user" },
    });

    function failedTurn(store: FeedStore, id = "web-a"): void {
      failingTurn(store, id);
      store.handleMessage(error(id, "provider returned 503"));
      store.handleMessage({ type: "turn_ended", reply_to: id });
    }

    it("replaces the failure with the rerun, under the same message", () => {
      const store = new FeedStore();
      failedTurn(store);
      store.askRetry("web-a");

      store.handleMessage(started("web-a"));

      expect(store.isProcessing).toBe(true);
      expect(store.activeTurnId).toBe("web-a");
      expect(store.feed.map((item) => item.kind)).toEqual(["user", "assistant"]);
    });

    it("shows the rerun's reply in the same turn as what the failed attempt did", () => {
      const store = new FeedStore();
      failedTurn(store);
      store.handleMessage(started("web-a"));
      store.handleMessage({
        type: "response",
        reply_to: "web-a",
        call: 0,
        endpoint: "ws",
        content: "Done: the index is tidy.",
      });
      store.handleMessage({ type: "turn_ended", reply_to: "web-a" });

      expect(store.feed.filter((item) => item.kind === "user")).toHaveLength(1);
      expect(store.feed.some((item) => item.kind === "turn-failure")).toBe(false);
      const entries = groupTurns(store.feed, store.activeTurnId);
      expect(entries.map((entry) => entry.kind)).toEqual(["single", "turn"]);
      const block = entries.at(-1);
      if (block?.kind !== "turn") throw new Error("expected the turn's block");
      expect(
        block.parts.map((part) => (part.kind === "message" ? part.item.kind : part.kind)),
      ).toEqual(["assistant", "assistant"]);
    });

    it("starts the rerun's tool calls in a group of their own", () => {
      const store = new FeedStore();
      const toolCall = (id: string): ServerMessage => ({
        type: "tool_call",
        reply_to: "web-a",
        call: 0,
        id,
        name: "exec",
        arguments: {},
        server: null,
      });
      store.pushUserMessage("Tidy the wiki index", undefined, "web-a");
      store.handleMessage(started("web-a"));
      store.handleMessage(toolCall("c1"));
      store.handleMessage(error("web-a", "provider returned 503"));
      store.handleMessage({ type: "turn_ended", reply_to: "web-a" });

      store.handleMessage(started("web-a"));
      store.handleMessage(toolCall("c2"));

      const groups = store.feed.filter((item) => item.kind === "tool-group");
      expect(groups.map((group) => group.calls.map((call) => call.id))).toEqual([["c1"], ["c2"]]);
    });

    it("asks again whether the turn changed anything once the rerun ends", () => {
      const store = new FeedStore();
      failedTurn(store);
      const asked = store.feed.find((item) => item.kind === "user");
      if (asked?.kind !== "user" || asked.turn === undefined) throw new Error("expected a tag");
      // The failed attempt's check found nothing changed.
      asked.turn.changed = false;

      store.handleMessage(started("web-a"));
      store.handleMessage({ type: "turn_ended", reply_to: "web-a" });

      expect(asked.turn).toEqual({ turnId: "web-a", changed: null });
    });

    it("leaves the failures of other turns where they are", () => {
      const store = new FeedStore();
      failedTurn(store, "web-a");
      failedTurn(store, "web-b");

      store.handleMessage(started("web-b"));

      expect(store.feed.filter((item) => item.kind === "turn-failure")).toMatchObject([
        { turnId: "web-a" },
      ]);
    });

    it("leaves the rerun's own failure with Try again, should it fail too", () => {
      const store = new FeedStore();
      failedTurn(store);
      store.handleMessage(started("web-a"));
      store.handleMessage(error("web-a", "provider returned 504"));
      store.handleMessage({ type: "turn_ended", reply_to: "web-a" });

      const failures = store.feed.filter((item) => item.kind === "turn-failure");
      expect(failures).toMatchObject([{ details: "provider returned 504", retry: {} }]);
    });

    it("waits for the agent's answer once asked, and offers Try again again if the connection drops first", () => {
      const store = new FeedStore();
      failedTurn(store);

      store.askRetry("web-a");
      expect(store.feed.at(-1)).toMatchObject({ retry: { asked: true } });

      store.forgetAsks();
      expect(store.feed.at(-1)).toMatchObject({ retry: {} });
      expect(store.feed.at(-1)).not.toHaveProperty("retry.asked");
    });

    it("gives the agent's reason on the failure when it won't rerun the turn", () => {
      const store = new FeedStore();
      failedTurn(store);
      store.askRetry("web-a");
      const reason = "That message can't be retried anymore. Send it again to try once more.";

      store.handleMessage({ type: "retry_refused", id: "web-a", message: reason });

      expect(store.feed.at(-1)).toMatchObject({
        kind: "turn-failure",
        message: "The model provider didn't answer.",
        retry: { refused: reason },
      });
      expect(store.feed.at(-1)).not.toHaveProperty("retry.asked");
    });

    it("keeps the user's message and the turn's state when the agent refuses", () => {
      const store = new FeedStore();
      failedTurn(store);
      const before = store.feed.map((item) => item.kind);

      store.handleMessage({ type: "retry_refused", id: "web-a", message: "Not any more." });

      expect(store.feed.map((item) => item.kind)).toEqual(before);
      expect(store.feed.filter((item) => item.kind === "user")).toHaveLength(1);
      expect(store.isProcessing).toBe(false);
    });

    it("ignores a refusal that names a turn the page shows no failure for", () => {
      const store = new FeedStore();
      failedTurn(store);

      store.handleMessage({ type: "retry_refused", id: "web-zzz", message: "Not any more." });

      expect(store.feed.at(-1)).toMatchObject({ retry: {} });
    });
  });

  it("shows the files of a message it sent as the agent saved them", () => {
    const store = new FeedStore();
    const sent = {
      item_id: "",
      filename: "notes.pdf",
      size: 10,
      content_type: "application/pdf",
      path: "",
    };
    store.pushUserMessage("Read this", [sent], "web-a");
    expect(store.feed.at(-1)).toMatchObject({ kind: "user", attachments: [sent] });

    const saved = {
      ...sent,
      item_id: "20260314_chat_attachment_notes_pdf",
      path: "inbox/agent/attachments/20260314_chat_attachment_notes_pdf/notes.pdf",
    };
    store.handleMessage({
      type: "user_message",
      id: "web-a",
      turn_id: "web-a",
      content: "Read this",
      attachments: [saved],
      endpoint: "ws",
    });
    const shown = store.feed.filter((item) => item.kind === "user");
    expect(shown).toHaveLength(1);
    expect(shown[0]).toMatchObject({ attachments: [saved] });
  });

  it("takes a refused message off the feed, and the reply it was waiting on with it", () => {
    const store = new FeedStore();
    store.pushUserMessage("Read this", undefined, "web-a");
    expect(store.isProcessing).toBe(true);

    store.withdrawRefused("web-a");

    expect(store.feed.filter((item) => item.kind === "user")).toEqual([]);
    expect(store.isProcessing).toBe(false);
  });

  it("takes a refused message off the waiting messages", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.pushUserMessage("And this", undefined, "web-b");
    expect(store.waiting.map((message) => message.id)).toEqual(["web-b"]);

    store.withdrawRefused("web-b");

    expect(store.waiting).toEqual([]);
    expect(store.isProcessing).toBe(true);
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
    store.handleMessage({ type: "error", reply_to: "t1", message: "Nope." });
    expect(store.announcement?.text).toBe("atlas couldn't finish");
  });

  it("leaves an error that names no turn to its toast", () => {
    const store = named();
    store.handleMessage({ type: "error", reply_to: null, message: "Bad frame." });
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
  it("flags a user message the running turn read, and not one that starts a turn", () => {
    const store = storeWithSettledHistory();
    store.pushUserMessage("Draft the post", undefined, "web-a");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.pushUserMessage("Keep it short", undefined, "web-b");
    store.handleMessage({
      type: "user_message",
      id: "web-b",
      turn_id: "web-a",
      content: "Keep it short",
      endpoint: "ws",
    });

    const users = store.feed.filter((item) => item.kind === "user").slice(-2);
    expect(users[0]).not.toHaveProperty("midTurn");
    expect(users[1]).toMatchObject({ content: "Keep it short", turnId: "web-a", midTurn: true });
  });

  it("flags a session's message the running turn read, and not one that starts a turn", () => {
    const store = storeWithSettledHistory();
    // As the backend announces a session's message: header first, with the session and its run.
    const fromSession = (id: string, turnId: string, body: string): ServerMessage => ({
      type: "user_message",
      id,
      turn_id: turnId,
      content: `[Agent Message from spawned-1 (spawned)]\n${body}`,
      agent_sender: { address: "spawned-1", category: "spawned", run_id: "run-1" },
      endpoint: "background",
    });
    store.handleMessage(fromSession("bg-1", "bg-1", "Done early"));
    store.handleMessage({
      type: "turn_started",
      reply_to: "bg-1",
      origin: { endpoint: "background", visibility: "background" },
    });
    store.handleMessage({ type: "turn_ended", reply_to: "bg-1" });
    store.pushUserMessage("Go", undefined, "web-a");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-a",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage(fromSession("bg-2", "web-a", "Result"));

    const messages = store.feed.filter((item) => item.kind === "agent-message");
    expect(messages[0]).toMatchObject({ content: "Done early", turnId: "bg-1" });
    expect(messages[0]).not.toHaveProperty("midTurn");
    expect(messages[1]).toMatchObject({
      from: "spawned-1",
      category: "spawned",
      content: "Result",
      runId: "run-1",
      messageId: "bg-2",
      turnId: "web-a",
      midTurn: true,
    });
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
