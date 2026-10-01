import { describe, expect, it } from "vitest";
import { FeedStore } from "../lib/feed.svelte";
import { convertHistoryMessages } from "../lib/feed-items";
import type { FeedItem, RecentMessage } from "../lib/types";
import { groupTurns, type FeedEntry } from "./turns";

/** Each entry in a line of text: a single item's kind and text, or a turn's tools and items. */
function describeEntries(entries: FeedEntry[]): string[] {
  const text = (item: FeedItem): string => {
    if (item.kind === "user" || item.kind === "assistant" || item.kind === "agent-message") {
      return `${item.kind}:${item.content}`;
    }
    return item.kind === "divider" ? `divider:${item.label}` : item.kind;
  };
  return entries.map((entry) =>
    entry.kind === "single"
      ? text(entry.item)
      : `turn[${entry.calls.map((call) => call.name).join(",")}](${entry.items.map(text).join(" | ")})`,
  );
}

function message(
  role: RecentMessage["role"],
  content: string,
  extra: Partial<RecentMessage> = {},
): RecentMessage {
  return { role, content, timestamp: "2026-03-14T12:00", visibility: "user", ...extra };
}

function toolCall(id: string, name: string, turnId?: string): RecentMessage[] {
  const ofTurn = turnId === undefined ? {} : { turn_id: turnId };
  return [
    message("assistant", "", { tool_calls: [{ id, name, arguments: {} }], ...ofTurn }),
    message("tool", "done", { tool_call_id: id, ...ofTurn }),
  ];
}

describe("turns in recent history", () => {
  it("bound a turn by its id, with every tool call first and its texts in order", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Tidy the wiki", { turn_id: "t1" }),
        ...toolCall("a", "memory_search", "t1"),
        message("assistant", "Looking at the index first.", { turn_id: "t1" }),
        ...toolCall("b", "read_file", "t1"),
        message("assistant", "Done: three pages merged.", { turn_id: "t1" }),
        message("user", "Thanks", { turn_id: "t2" }),
        message("assistant", "Any time.", { turn_id: "t2" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Tidy the wiki",
      "turn[memory_search,read_file](assistant:Looking at the index first. | assistant:Done: three pages merged.)",
      "user:Thanks",
      "turn[](assistant:Any time.)",
    ]);
  });

  it("splits back-to-back turns on a change of id even with no message between", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Two things", { turn_id: "t1" }),
        message("assistant", "First.", { turn_id: "t1" }),
        message("assistant", "Second.", { turn_id: "t2" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Two things",
      "turn[](assistant:First.)",
      "turn[](assistant:Second.)",
    ]);
  });

  it("keeps a message sent mid-turn inside the turn it reached", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Draft the post", { turn_id: "t1" }),
        message("assistant", "Drafting now.", { turn_id: "t1" }),
        message("user", "Keep it short", { turn_id: "t1" }),
        ...toolCall("a", "write_file", "t1"),
        message("assistant", "Saved a short draft.", { turn_id: "t1" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Draft the post",
      "turn[write_file](assistant:Drafting now. | user:Keep it short | assistant:Saved a short draft.)",
    ]);
  });

  it("falls back to user messages and agent messages where messages carry no id", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Hello"),
        message("assistant", "Hi."),
        message("user", "Result of the research", {
          agent_sender: { address: "spawned-1", category: "spawned" },
        }),
        ...toolCall("a", "memory_add"),
        message("assistant", "Noted the result."),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Hello",
      "turn[](assistant:Hi.)",
      "agent-message:Result of the research",
      "turn[memory_add](assistant:Noted the result.)",
    ]);
  });
});

describe("turns in episodes", () => {
  it("bound turns by user messages, as episodes carry no ids", () => {
    const items = convertHistoryMessages(
      [
        message("user", "What changed?"),
        ...toolCall("a", "list_files"),
        message("assistant", "Two files."),
        message("user", "And today?"),
        message("assistant", "Nothing yet."),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:What changed?",
      "turn[list_files](assistant:Two files.)",
      "user:And today?",
      "turn[](assistant:Nothing yet.)",
    ]);
  });

  it("starts a new block after a divider", () => {
    const store = new FeedStore();
    store.prependEpisode({
      kind: "episode",
      episode_id: "ep-001",
      date: "2026-03-13",
      messages: [message("user", "Hi"), message("assistant", "Hello.")],
      next_cursor: null,
    });
    expect(describeEntries(groupTurns(store.feed, null))).toEqual([
      "divider:ep-001 · 2026-03-13",
      "user:Hi",
      "turn[](assistant:Hello.)",
      "compressed-marker",
    ]);
  });
});

describe("live turns", () => {
  it("bound a turn by turn_started and turn_ended, and mark it live meanwhile", () => {
    const store = new FeedStore();
    store.pushUserMessage("Check the routing doc");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    store.handleMessage({ type: "broadcast_response", content: "Looking first." });
    store.handleMessage({
      type: "tool_call",
      id: "c1",
      name: "memory_search",
      arguments: "{}",
    });

    let entries = groupTurns(store.feed, store.activeTurnId);
    expect(describeEntries(entries)).toEqual([
      "user:Check the routing doc",
      "turn[memory_search](assistant:Looking first.)",
    ]);
    expect(entries[1]).toMatchObject({ kind: "turn", turnId: "m1", live: true });

    store.handleMessage({ type: "response", reply_to: "m1", content: "It's tidy." });
    store.handleMessage({ type: "turn_ended", reply_to: "m1" });
    entries = groupTurns(store.feed, store.activeTurnId);
    expect(describeEntries(entries)).toEqual([
      "user:Check the routing doc",
      "turn[memory_search](assistant:Looking first. | assistant:It's tidy.)",
    ]);
    expect(entries[1]).toMatchObject({ live: false });
  });

  it("keeps the next turn's tool calls out of the turn before it", () => {
    const store = new FeedStore();
    store.pushUserMessage("First");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    store.handleMessage({ type: "tool_call", id: "c1", name: "read_file", arguments: "{}" });
    store.handleMessage({ type: "turn_ended", reply_to: "m1" });
    store.handleMessage({ type: "turn_started", reply_to: "m2" });
    store.handleMessage({ type: "tool_call", id: "c2", name: "write_file", arguments: "{}" });

    expect(describeEntries(groupTurns(store.feed, store.activeTurnId))).toEqual([
      "user:First",
      "turn[read_file]()",
      "turn[write_file]()",
    ]);
  });

  it("puts a message sent while the turn runs inside it", () => {
    const store = new FeedStore();
    store.pushUserMessage("Draft the post");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    store.handleMessage({ type: "broadcast_response", content: "Drafting." });
    store.pushUserMessage("Keep it short");
    store.handleMessage({ type: "response", reply_to: "m1", content: "Done, and short." });

    expect(describeEntries(groupTurns(store.feed, store.activeTurnId))).toEqual([
      "user:Draft the post",
      "turn[](assistant:Drafting. | user:Keep it short | assistant:Done, and short.)",
    ]);
  });

  it("shows the turn in flight after its message before it has output, under one key", () => {
    const store = new FeedStore();
    store.pushUserMessage("Plan the week");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    let entries = groupTurns(store.feed, store.activeTurnId);
    expect(describeEntries(entries)).toEqual(["user:Plan the week", "turn[]()"]);
    expect(entries[1]).toMatchObject({ key: "turn:m1", live: true });

    store.handleMessage({ type: "tool_call", id: "c1", name: "read_file", arguments: "{}" });
    entries = groupTurns(store.feed, store.activeTurnId);
    expect(entries[1]).toMatchObject({ key: "turn:m1", live: true });
  });

  it("keeps a block for a turn that ended with nothing to show only when asked", () => {
    const store = new FeedStore();
    store.pushUserMessage("Never mind");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    store.askStop();
    store.handleMessage({ type: "turn_ended", reply_to: "m1" });
    store.pushUserMessage("Something else");

    expect(describeEntries(groupTurns(store.feed, null))).toEqual([
      "user:Never mind",
      "user:Something else",
    ]);
    const stopped = (id: string): boolean => store.observed.get(id)?.ending === "stopped";
    expect(describeEntries(groupTurns(store.feed, null, stopped))).toEqual([
      "user:Never mind",
      "turn[]()",
      "user:Something else",
    ]);
  });

  it("marks only the latest block of the turn in flight live", () => {
    const store = new FeedStore();
    store.pushUserMessage("Go");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    store.handleMessage({ type: "broadcast_response", content: "First part." });
    store.pushLocalSystem("A note in between.");
    store.handleMessage({ type: "broadcast_response", content: "Second part." });

    const blocks = groupTurns(store.feed, store.activeTurnId).filter((e) => e.kind === "turn");
    expect(blocks.map((b) => b.live)).toEqual([false, true]);
  });

  it("ends a turn the agent stopped in the middle of", () => {
    const store = new FeedStore();
    store.pushUserMessage("Long job");
    store.handleMessage({ type: "turn_started", reply_to: "m1" });
    store.handleMessage({ type: "broadcast_response", content: "Starting." });

    store.abandonLiveTurn();
    expect(store.isProcessing).toBe(false);
    expect(store.activeTurnId).toBeNull();
    expect(groupTurns(store.feed, store.activeTurnId).at(-1)).toMatchObject({ live: false });
  });
});
