import { describe, expect, it } from "vitest";
import { groupTurns } from "../feed/turns";
import { FeedStore } from "./feed.svelte";
import type { FeedItem, ServerMessage } from "./types";

// The messages the agent hasn't read yet: one sent while a turn runs waits
// apart from the conversation, and joins it where the agent reads it.

function setup(): FeedStore {
  const store = new FeedStore(
    () => null,
    () => "atlas",
    (flush) => {
      flush();
    },
  );
  store.connected();
  return store;
}

function started(replyTo: string): ServerMessage {
  return {
    type: "turn_started",
    reply_to: replyTo,
    origin: { endpoint: "ws", visibility: "user" },
  };
}

function read(id: string, turnId: string, content = "also check the logs"): ServerMessage {
  return { type: "user_message", id, turn_id: turnId, content, endpoint: "ws" };
}

function toolCall(turnId: string, id: string): ServerMessage {
  return {
    type: "tool_call",
    reply_to: turnId,
    call: 0,
    id,
    name: "read_file",
    arguments: {},
    server: null,
  };
}

function snapshot(queued: Array<{ id: string; content: string }>): ServerMessage {
  return {
    type: "turn_snapshot",
    turn: null,
    queued: queued.map((message) => ({ ...message, endpoint: "ws" })),
  };
}

function kinds(items: readonly FeedItem[]): string[] {
  return items.map((item) => item.kind);
}

/** A turn `t1` this page started, running. */
function running(): FeedStore {
  const store = setup();
  store.pushUserMessage("look into the outage", undefined, "t1");
  store.handleMessage(read("t1", "t1", "look into the outage"));
  store.handleMessage(started("t1"));
  return store;
}

describe("a message sent while a turn runs", () => {
  it("waits apart from the conversation instead of joining the turn", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    expect(kinds(store.feed)).toEqual(["user"]);
    expect(store.waiting).toMatchObject([{ id: "m2", content: "also check the logs" }]);
  });

  it("joins the turn where the agent reads it, after the steps before that", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.handleMessage(toolCall("t1", "c1"));
    store.handleMessage(read("m2", "t1"));

    expect(store.waiting).toEqual([]);
    expect(kinds(store.feed)).toEqual(["user", "tool-group", "user"]);
    expect(store.feed[2]).toMatchObject({ content: "also check the logs", midTurn: true });
    const [, turn] = groupTurns(store.feed, store.activeTurnId);
    expect(turn?.kind).toBe("turn");
  });

  it("starts the next turn when the agent reads it only after the turn ends", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.handleMessage({ type: "turn_ended", reply_to: "t1" });
    store.handleMessage(read("m2", "m2"));
    store.handleMessage(started("m2"));
    store.handleMessage({ type: "response", reply_to: "m2", endpoint: "ws", content: "Checked." });

    expect(store.waiting).toEqual([]);
    const entries = groupTurns(store.feed, store.activeTurnId);
    expect(entries.map((entry) => entry.kind)).toEqual(["single", "single", "turn"]);
    const [, second] = entries;
    expect(second).toMatchObject({ kind: "single", item: { content: "also check the logs" } });
  });

  it("goes when another page takes it back", () => {
    const store = running();
    store.pushUserMessage("never mind", undefined, "m2");
    store.handleMessage({ type: "message_withdrawn", id: "m2" });
    expect(store.waiting).toEqual([]);
  });

  it("shows a waiting message another page or interface sent, once", () => {
    const store = running();
    const queued: ServerMessage = {
      type: "message_queued",
      message: { id: "tg-1", content: "from my phone", endpoint: "telegram" },
    };
    store.handleMessage(queued);
    store.handleMessage(queued);
    expect(store.waiting).toMatchObject([{ id: "tg-1", endpoint: "telegram" }]);
  });

  it("shows the files a message sent while a turn ran came with, from the agent's frame", () => {
    const store = running();
    const file = {
      item_id: "20260314_chat_attachment_notes_md",
      filename: "notes.md",
      size: 120,
      content_type: "text/markdown",
      path: "inbox/agent/attachments/20260314_chat_attachment_notes_md/notes.md",
    };
    store.handleMessage({
      type: "message_queued",
      message: { id: "m3", content: "see the notes", endpoint: "ws", attachments: [file] },
    });
    expect(store.waiting).toMatchObject([{ id: "m3", attachments: [file] }]);
  });

  it("forgets what was asked of it when the agent says it no longer waits", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.askOfWaiting("m2", "send-now");
    expect(store.waiting[0]?.asked).toBe("send-now");
    store.handleMessage({ type: "queued_message_unavailable", id: "m2" });
    expect(store.waiting[0]?.asked).toBeUndefined();
  });

  it("is still sent as a turn of its own when no turn runs", () => {
    const store = setup();
    store.pushUserMessage("hello", undefined, "m1");
    expect(store.waiting).toEqual([]);
    expect(kinds(store.feed)).toEqual(["user"]);
  });
});

describe("waiting messages across a reconnect", () => {
  it("takes the snapshot's list, adding what other pages sent", () => {
    const store = running();
    store.handleMessage(snapshot([{ id: "web-other", content: "from another tab" }]));
    expect(store.waiting.map((message) => message.id)).toEqual(["web-other"]);
  });

  it("drops a message sent before the reconnect that the snapshot no longer lists", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.connected();
    store.handleMessage(snapshot([]));
    expect(store.waiting).toEqual([]);
  });

  it("keeps a message sent on this connection that the snapshot may not have caught yet", () => {
    const store = running();
    store.connected();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.handleMessage(snapshot([]));
    expect(store.waiting.map((message) => message.id)).toEqual(["m2"]);
  });

  it("keeps a message held offline, which goes out after the snapshot is asked for", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2", false);
    store.connected();
    store.handleMessage(snapshot([]));
    expect(store.waiting.map((message) => message.id)).toEqual(["m2"]);
  });

  it("joins the turn a snapshot shows it was read into", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.connected();
    store.handleMessage({
      type: "turn_snapshot",
      queued: [],
      turn: {
        reply_to: "t1",
        started_at: "2026-10-12T10:00:00Z",
        now: "2026-10-12T10:00:05Z",
        frames: [
          { at: "2026-10-12T10:00:00Z", frame: read("t1", "t1", "look into the outage") },
          { at: "2026-10-12T10:00:00Z", frame: started("t1") },
          { at: "2026-10-12T10:00:02Z", frame: toolCall("t1", "c1") },
          { at: "2026-10-12T10:00:03Z", frame: read("m2", "t1") },
        ],
      },
    });
    expect(store.waiting).toEqual([]);
    expect(kinds(store.feed)).toEqual(["user", "tool-group", "user"]);
  });

  it("goes when the agent stops, which puts it in history", () => {
    const store = running();
    store.pushUserMessage("also check the logs", undefined, "m2");
    store.abandonLiveTurn();
    expect(store.waiting).toEqual([]);
  });
});
