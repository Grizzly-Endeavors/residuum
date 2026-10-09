import { describe, expect, it } from "vitest";
import { groupTurns } from "../feed/turns";
import { FeedStore } from "./feed.svelte";
import type { FeedItem, MessageSender, RecentMessage, ServerMessage } from "./types";

// What the main chat's feed shows of turns as the agent's socket carries them:
// the people behind messages from any channel, text and reasoning streaming
// in, and where replies were delivered.

/** A store whose streamed pieces are added when the test says a frame has passed. */
function setup(): { store: FeedStore; frame: () => void } {
  const queued: Array<() => void> = [];
  const store = new FeedStore(
    () => null,
    () => "atlas",
    (flush) => queued.push(flush),
  );
  return {
    store,
    frame: () => {
      for (const flush of queued.splice(0)) flush();
    },
  };
}

const origin = { endpoint: "ws", visibility: "user" } as const;
const ALEX: MessageSender = {
  name: "Alex",
  id: "42",
  interface: "telegram",
  location: "direct message",
};

function started(replyTo: string, endpoint = "ws"): ServerMessage {
  return { type: "turn_started", reply_to: replyTo, origin: { ...origin, endpoint } };
}

function echo(
  id: string,
  content: string,
  more: Partial<Extract<ServerMessage, { type: "user_message" }>> = {},
): ServerMessage {
  return { type: "user_message", id, turn_id: id, content, endpoint: "ws", ...more };
}

function kinds(items: readonly FeedItem[]): string[] {
  return items.map((item) => item.kind);
}

describe("people's messages from any channel", () => {
  it("shows a message another page sent, as the start of the turn that follows", () => {
    const { store } = setup();
    store.handleMessage(echo("web-other", "Check the wiki"));
    store.handleMessage(started("web-other"));
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "web-other",
      call: 0,
      content: "Looking.",
    });

    expect(kinds(store.feed)).toEqual(["user", "assistant"]);
    expect(store.feed[0]).toMatchObject({ content: "Check the wiki", turnId: "web-other" });
    expect(store.feed[0]).not.toHaveProperty("midTurn");
    expect(store.isProcessing).toBe(true);
    const entries = groupTurns(store.feed, store.activeTurnId);
    expect(entries.map((entry) => entry.kind)).toEqual(["single", "turn"]);
  });

  it("doesn't show this page's own message a second time when the agent echoes it", () => {
    const { store } = setup();
    store.pushUserMessage("Check the wiki", undefined, "web-mine");
    store.handleMessage(echo("web-mine", "Check the wiki"));
    store.handleMessage(started("web-mine"));
    expect(store.feed.filter((item) => item.kind === "user")).toHaveLength(1);
  });

  it("shows a message from a chat interface with the person who sent it", () => {
    const { store } = setup();
    store.handleMessage(
      echo("tg-1", "Can you check the wiki?", { endpoint: "telegram", sender: ALEX }),
    );
    store.handleMessage(started("tg-1", "telegram"));
    expect(store.feed[0]).toMatchObject({
      kind: "user",
      content: "Can you check the wiki?",
      sender: ALEX,
      turnId: "tg-1",
    });
  });

  it("keeps the images of a message another page sent", () => {
    const { store } = setup();
    const image = { media_type: "image/png", data: "AAAA" };
    store.handleMessage(echo("web-other", "What is this?", { images: [image] }));
    expect(store.feed[0]).toMatchObject({ images: [image] });
  });

  it("puts a message that joins the turn in flight inside it", () => {
    const { store } = setup();
    store.handleMessage(echo("tg-1", "Draft the post", { endpoint: "telegram", sender: ALEX }));
    store.handleMessage(started("tg-1", "telegram"));
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "tg-1",
      call: 0,
      content: "Drafting.",
    });
    store.handleMessage(echo("web-2", "Keep it short", { turn_id: "tg-1", endpoint: "ws" }));

    expect(store.feed.at(-1)).toMatchObject({
      content: "Keep it short",
      turnId: "tg-1",
      midTurn: true,
    });
    const block = groupTurns(store.feed, store.activeTurnId).at(-1);
    if (block?.kind !== "turn") throw new Error("expected the turn's block");
    expect(
      block.parts.map((part) => (part.kind === "message" ? part.item.kind : part.kind)),
    ).toEqual(["assistant", "user"]);
  });

  it("shows a turn from a chat interface once when history records it while its frames still arrive", () => {
    const { store, frame } = setup();
    store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    store.handleMessage(echo("tg-1", "Check the wiki", { endpoint: "telegram", sender: ALEX }));
    store.handleMessage(started("tg-1", "telegram"));
    store.handleMessage({ type: "text_delta", reply_to: "tg-1", call: 0, text: "Look" });
    const message = (role: "user" | "assistant", content: string): RecentMessage => ({
      role,
      content,
      timestamp: "2026-01-01T10:00",
      visibility: "user",
      turn_id: "tg-1",
      ...(role === "user" ? { sender: ALEX } : {}),
    });
    store.reloadHistory({
      kind: "recent",
      messages: [message("user", "Check the wiki"), message("assistant", "Looked.")],
      next_cursor: null,
    });

    // The rest of the turn's frames are history's now.
    store.handleMessage({ type: "text_delta", reply_to: "tg-1", call: 0, text: "ing" });
    store.handleMessage({
      type: "response",
      reply_to: "tg-1",
      call: 0,
      endpoint: "telegram",
      content: "Looked.",
    });
    store.handleMessage({ type: "turn_ended", reply_to: "tg-1" });
    frame();

    expect(store.feed.filter((item) => item.kind === "user")).toHaveLength(1);
    expect(store.feed.filter((item) => item.kind === "assistant")).toMatchObject([
      { content: "Looked." },
    ]);
    expect(store.isProcessing).toBe(false);
  });
});

describe("where a reply was delivered", () => {
  it("is noted for a reply sent to a chat interface", () => {
    const { store } = setup();
    store.handleMessage(started("tg-1", "telegram"));
    store.handleMessage({
      type: "response",
      reply_to: "tg-1",
      call: 1,
      endpoint: "telegram",
      content: "Done.",
    });
    expect(store.feed.at(-1)).toMatchObject({ kind: "assistant", deliveredTo: "telegram" });
  });

  it("is left out for a reply to this page, to nowhere, or to the background", () => {
    for (const endpoint of ["ws", "", "background"]) {
      const { store } = setup();
      store.handleMessage(started("t1"));
      store.handleMessage({
        type: "response",
        reply_to: "t1",
        call: 0,
        endpoint,
        content: "Done.",
      });
      expect(store.feed.at(-1)).not.toHaveProperty("deliveredTo");
    }
  });

  it("is noted on the text that streamed in", () => {
    const { store, frame } = setup();
    store.handleMessage(started("tg-1", "telegram"));
    store.handleMessage({ type: "text_delta", reply_to: "tg-1", call: 0, text: "Do" });
    store.handleMessage({ type: "text_delta", reply_to: "tg-1", call: 0, text: "ne" });
    store.handleMessage({
      type: "response",
      reply_to: "tg-1",
      call: 0,
      endpoint: "telegram",
      content: "Done.",
    });
    frame();
    expect(store.feed.filter((item) => item.kind === "assistant")).toMatchObject([
      { content: "Done.", deliveredTo: "telegram", streaming: false },
    ]);
  });
});

describe("a turn no person started", () => {
  it("shows the reply of a teammate's message once, whole, and ends the turn", () => {
    // What the backend sends: no frame for the message itself, the reply in pieces and then
    // whole, to nowhere (an empty endpoint).
    const { store, frame } = setup();
    store.handleMessage({
      type: "turn_started",
      reply_to: "bg-1",
      origin: { endpoint: "background", visibility: "background" },
    });
    store.handleMessage({ type: "text_delta", reply_to: "bg-1", call: 0, text: "scout asked " });
    store.handleMessage({ type: "text_delta", reply_to: "bg-1", call: 0, text: "me to look." });
    store.handleMessage({
      type: "response",
      reply_to: "bg-1",
      call: 0,
      endpoint: "",
      content: "scout asked me to look.",
    });
    store.handleMessage({ type: "turn_ended", reply_to: "bg-1" });
    frame();

    expect(store.feed).toMatchObject([
      { kind: "assistant", content: "scout asked me to look.", turnId: "bg-1", streaming: false },
    ]);
    expect(store.feed[0]).not.toHaveProperty("deliveredTo");
    expect(store.activeTurnId).toBeNull();
    expect(store.isProcessing).toBe(false);
  });
});

describe("a reply posted to this page outside any turn", () => {
  it("shows as a message of its own, and ends nothing", () => {
    const { store } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage({
      type: "response",
      reply_to: "",
      endpoint: "ws",
      content: "Heads up: the build is green.",
    });

    expect(store.isProcessing).toBe(true);
    expect(store.activeTurnId).toBe("t1");
    expect(store.feed.at(-1)).toMatchObject({
      kind: "assistant",
      content: "Heads up: the build is green.",
    });
    expect(store.feed.at(-1)).not.toHaveProperty("turnId");
  });

  it("is a message of its own when no turn runs", () => {
    const { store } = setup();
    store.handleMessage({
      type: "response",
      reply_to: "",
      endpoint: "ws",
      content: "Pulse found nothing.",
    });
    expect(store.activeTurnId).toBeNull();
    expect(kinds(store.feed)).toEqual(["assistant"]);
    expect(store.announcement?.text).toBe("atlas replied: Pulse found nothing.");
  });
});

describe("text streaming in", () => {
  const delta = (call: number, text: string): ServerMessage => ({
    type: "text_delta",
    reply_to: "t1",
    call,
    text,
  });

  it("grows a draft once per frame, which the authoritative text then replaces", () => {
    const { store, frame } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage(delta(0, "Let"));
    store.handleMessage(delta(0, " me"));
    store.handleMessage(delta(0, " look"));
    expect(store.feed[0]).toMatchObject({ content: "Let", streaming: true });
    frame();
    expect(store.feed[0]).toMatchObject({ content: "Let me look", streaming: true });

    store.handleMessage({
      type: "broadcast_response",
      reply_to: "t1",
      call: 0,
      content: "Let me look.",
    });
    expect(store.feed).toHaveLength(1);
    expect(store.feed[0]).toMatchObject({ content: "Let me look.", streaming: false });
  });

  it("is never announced, only the reply once it is complete", () => {
    const { store, frame } = setup();
    store.handleMessage(started("t1"));
    const working = store.announcement;
    store.handleMessage(delta(0, "Done"));
    store.handleMessage(delta(0, "."));
    frame();
    expect(store.announcement).toBe(working);

    store.handleMessage({
      type: "response",
      reply_to: "t1",
      call: 0,
      endpoint: "ws",
      content: "Done.",
    });
    expect(store.announcement?.text).toBe("atlas replied: Done.");
  });

  it("joins a turn the page connected to while it streamed", () => {
    const { store } = setup();
    store.handleMessage(delta(0, "Halfway"));
    expect(store.activeTurnId).toBe("t1");
    expect(store.isProcessing).toBe(true);
    expect(store.observed.get("t1")?.gaps).toEqual([0]);
  });

  it("starts over on a restart, saying so until the new attempt arrives", () => {
    const { store, frame } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage(delta(0, "Let me"));
    store.handleMessage(delta(0, " look"));
    store.handleMessage({ type: "stream_restart", reply_to: "t1", call: 0 });
    frame();
    expect(kinds(store.feed)).toEqual([]);
    expect(store.observed.get("t1")?.retrying).toBe(true);

    store.handleMessage(delta(0, "I'll check"));
    expect(store.observed.get("t1")?.retrying).toBe(false);
    expect(store.feed[0]).toMatchObject({ content: "I'll check", streaming: true });
  });

  it("keeps the partial text when the user stops it, and says it was stopped", () => {
    const { store } = setup();
    store.pushUserMessage("Write it", undefined, "t1");
    store.handleMessage(started("t1"));
    store.handleMessage(delta(0, "The port is"));
    store.handleMessage(delta(0, " set twice"));
    store.askStop();
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });

    const reply = store.feed.at(-1);
    expect(reply).toMatchObject({
      kind: "assistant",
      content: "The port is set twice",
      streaming: false,
      cut: "stopped",
    });
    expect(store.observed.get("t1")?.retrying).toBe(false);
  });

  it("says the agent stopped under it, when it did", () => {
    const { store } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage(delta(0, "The port"));
    store.abandonLiveTurn();
    expect(store.feed.at(-1)).toMatchObject({
      content: "The port",
      cut: "interrupted",
      streaming: false,
    });
  });

  it("says a failure cut it short", () => {
    const { store } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage(delta(0, "The port"));
    store.handleMessage({ type: "error", reply_to: "t1", message: "Nope.", details: null });
    expect(store.feed.find((item) => item.kind === "assistant")).toMatchObject({
      cut: "interrupted",
    });
  });

  it("leaves a frame of a turn history settled to history", () => {
    const { store, frame } = setup();
    store.loadHistory({ kind: "recent", messages: [], next_cursor: null });
    store.pushUserMessage("Hi", undefined, "t1");
    store.handleMessage(started("t1"));
    store.reloadHistory({
      kind: "recent",
      messages: [
        {
          role: "user",
          content: "Hi",
          timestamp: "2026-01-01T10:00",
          visibility: "user",
          turn_id: "t1",
        },
        {
          role: "assistant",
          content: "Hello.",
          timestamp: "2026-01-01T10:00",
          visibility: "user",
          turn_id: "t1",
        },
      ],
      next_cursor: null,
    });
    store.handleMessage(delta(0, "Hel"));
    frame();
    expect(store.feed.filter((item) => item.kind === "assistant")).toMatchObject([
      { content: "Hello." },
    ]);
  });
});

describe("reasoning streaming in", () => {
  it("shows as a step while it streams, then once it is done", () => {
    const { store, frame } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage({
      type: "thinking_delta",
      reply_to: "t1",
      call: 0,
      text: "The user wants",
    });
    store.handleMessage({ type: "thinking_delta", reply_to: "t1", call: 0, text: " the config." });
    frame();
    expect(store.feed[0]).toMatchObject({
      kind: "thinking",
      content: "The user wants the config.",
      streaming: true,
    });

    store.handleMessage({
      type: "thinking",
      reply_to: "t1",
      call: 0,
      content: "The user wants the config file.",
    });
    expect(store.feed).toHaveLength(1);
    expect(store.feed[0]).toMatchObject({
      content: "The user wants the config file.",
      streaming: false,
    });
  });

  it("ends when the same call's text begins", () => {
    const { store } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage({ type: "thinking_delta", reply_to: "t1", call: 0, text: "hm" });
    store.handleMessage({ type: "text_delta", reply_to: "t1", call: 0, text: "Let me" });
    expect(store.feed[0]).toMatchObject({ kind: "thinking", streaming: false });
    expect(store.feed[1]).toMatchObject({ kind: "assistant", streaming: true });
  });

  it("goes in the activity before the text and the tools of the call that did it", () => {
    const { store } = setup();
    store.handleMessage(started("t1"));
    store.handleMessage({ type: "thinking", reply_to: "t1", call: 0, content: "Read the config." });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "t1",
      call: 0,
      content: "Reading.",
    });
    store.handleMessage({
      type: "tool_call",
      reply_to: "t1",
      call: 0,
      id: "c1",
      name: "read_file",
      arguments: {},
      server: null,
    });
    expect(kinds(store.feed)).toEqual(["thinking", "assistant", "tool-group"]);
  });

  it("keeps each model call's tool calls in a group of their own", () => {
    const { store } = setup();
    store.handleMessage(started("t1"));
    for (const [call, id] of [
      [0, "c1"],
      [0, "c2"],
      [1, "c3"],
    ] as const) {
      store.handleMessage({
        type: "tool_call",
        reply_to: "t1",
        call,
        id,
        name: "read_file",
        arguments: {},
        server: null,
      });
    }
    const groups = store.feed.filter((item) => item.kind === "tool-group");
    expect(groups.map((group) => group.calls.map((call) => call.id))).toEqual([
      ["c1", "c2"],
      ["c3"],
    ]);
  });
});
describe("catching up on history after a turn with reasoning", () => {
  it("doesn't show the reasoning and tool calls of a turn it saw live a second time", () => {
    const { store } = setup();
    const message = (more: Partial<RecentMessage>): RecentMessage => ({
      role: "user",
      content: "",
      timestamp: "2026-01-01T10:00",
      visibility: "user",
      ...more,
    });
    const earlier = [
      message({ content: "Earlier question" }),
      message({ role: "assistant", content: "Earlier answer" }),
    ];
    store.loadHistory({ kind: "recent", messages: earlier, next_cursor: null });

    store.pushUserMessage("Read the config", undefined, "t1");
    store.handleMessage(started("t1"));
    store.handleMessage({ type: "thinking", reply_to: "t1", call: 0, content: "Read it first." });
    store.handleMessage({
      type: "tool_call",
      reply_to: "t1",
      call: 0,
      id: "c1",
      name: "read_file",
      arguments: {},
      server: null,
    });
    store.askStop();
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    expect(kinds(store.feed).filter((kind) => kind !== "divider")).toEqual([
      "user",
      "assistant",
      "user",
      "thinking",
      "tool-group",
    ]);

    const recorded = [
      ...earlier,
      message({ content: "Read the config", turn_id: "t1" }),
      message({
        role: "assistant",
        content: "",
        turn_id: "t1",
        thinking: ["Read it first."],
        tool_calls: [{ id: "c1", name: "read_file", arguments: {}, server: null }],
      }),
      message({ role: "tool", content: "ok", tool_call_id: "c1", turn_id: "t1" }),
    ];
    expect(store.reconcileRecent({ kind: "recent", messages: recorded, next_cursor: null })).toBe(
      true,
    );
    expect(kinds(store.feed).filter((kind) => kind !== "divider")).toEqual([
      "user",
      "assistant",
      "user",
      "thinking",
      "tool-group",
    ]);
  });
});
