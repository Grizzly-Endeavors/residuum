import { describe, expect, it } from "vitest";
import { FeedStore } from "../lib/feed.svelte";
import { convertHistoryMessages } from "../lib/feed-items";
import type { FeedItem, RecentMessage, ToolCallState } from "../lib/types";
import {
  drawnParts,
  gapsWithin,
  groupTurns,
  type ActivitySegment,
  type FeedEntry,
  type TurnPart,
} from "./turns";

/** Each entry in a line of text: a single item's kind and text, or a turn's tools and items. */
function describeEntries(entries: FeedEntry[]): string[] {
  const text = (item: FeedItem): string => {
    if (item.kind === "user" || item.kind === "assistant" || item.kind === "agent-message") {
      return `${item.kind}:${item.content}`;
    }
    return item.kind === "divider" ? `divider:${item.episode ?? item.label}` : item.kind;
  };
  const describePart = (part: TurnPart): string =>
    part.kind === "activity"
      ? `[${part.calls.map((call) => call.name).join(",")}]`
      : text(part.item);
  return entries.map((entry) =>
    entry.kind === "single"
      ? text(entry.item)
      : `turn(${entry.parts.map(describePart).join(" | ")})`,
  );
}

function message(
  role: RecentMessage["role"],
  content: string,
  extra: Partial<RecentMessage> = {},
): RecentMessage {
  return { role, content, timestamp: "2026-03-14T12:00", visibility: "user", ...extra };
}

function toolCall(
  id: string,
  name: string,
  turnId?: string,
  server?: string | null,
): RecentMessage[] {
  const ofTurn = turnId === undefined ? {} : { turn_id: turnId };
  return [
    message("assistant", "", {
      tool_calls: [{ id, name, arguments: {}, ...(server === undefined ? {} : { server }) }],
      ...ofTurn,
    }),
    message("tool", "done", { tool_call_id: id, ...ofTurn }),
  ];
}

describe("turns in recent history", () => {
  it("bound a turn by its id, with every tool call first and its texts in order", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Tidy the wiki", { turn_id: "t1" }),
        ...toolCall("a", "memory_search", "t1", "notes"),
        message("assistant", "Looking at the index first.", { turn_id: "t1" }),
        ...toolCall("b", "read_file", "t1", null),
        message("assistant", "Done: three pages merged.", { turn_id: "t1" }),
        message("user", "Thanks", { turn_id: "t2" }),
        message("assistant", "Any time.", { turn_id: "t2" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Tidy the wiki",
      "turn([memory_search] | assistant:Looking at the index first. | [read_file] | assistant:Done: three pages merged.)",
      "user:Thanks",
      "turn(assistant:Any time.)",
    ]);
    expect(
      items.filter((item) => item.kind === "tool-group").flatMap((item) => item.calls),
    ).toMatchObject([
      { name: "memory_search", server: "notes" },
      { name: "read_file", server: null },
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
      "turn(assistant:First.)",
      "turn(assistant:Second.)",
    ]);
  });

  it("keeps a message the agent took in mid-turn inside the turn it reached", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Draft the post", { turn_id: "t1" }),
        // The agent takes a waiting message in at its checkpoint after a tool batch.
        message("assistant", "Drafting now.", {
          turn_id: "t1",
          tool_calls: [{ id: "a", name: "write_file", arguments: {} }],
        }),
        message("tool", "done", { tool_call_id: "a", turn_id: "t1" }),
        message("system", "A note from the subconscious.", { turn_id: "t1" }),
        message("user", "Keep it short", { turn_id: "t1" }),
        message("assistant", "Saved a short draft.", { turn_id: "t1" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Draft the post",
      "turn(assistant:Drafting now. | [write_file] | user:Keep it short | assistant:Saved a short draft.)",
    ]);
  });

  it("keeps an agent's message that reached the turn mid-way inside it too", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Look into it", { turn_id: "t1" }),
        ...toolCall("a", "memory_search", "t1"),
        message("user", "Result of the research", {
          turn_id: "t1",
          agent_sender: { address: "spawned-1", category: "spawned" },
        }),
        message("assistant", "Thanks, that settles it.", { turn_id: "t1" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Look into it",
      "turn([memory_search] | agent-message:Result of the research | assistant:Thanks, that settles it.)",
    ]);
  });

  it("starts a new block at each message that starts a turn, though ids repeat", () => {
    // Every page load once counted its message ids from web-1 again, and the
    // agent kept them as turn ids.
    const items = convertHistoryMessages(
      [
        message("user", "First visit", { turn_id: "web-1" }),
        ...toolCall("a", "memory_search", "web-1"),
        message("assistant", "Found it.", { turn_id: "web-1" }),
        message("user", "Second visit", { turn_id: "web-1" }),
        message("assistant", "Welcome back.", { turn_id: "web-1" }),
        message("user", "And once more", { turn_id: "web-1" }),
        ...toolCall("b", "read_file", "web-1"),
        message("assistant", "Here it is.", { turn_id: "web-1" }),
      ],
      { mode: "main" },
    );
    const entries = groupTurns(items, null);
    expect(describeEntries(entries)).toEqual([
      "user:First visit",
      "turn([memory_search] | assistant:Found it.)",
      "user:Second visit",
      "turn(assistant:Welcome back.)",
      "user:And once more",
      "turn([read_file] | assistant:Here it is.)",
    ]);
    const keys = entries.map((entry) => entry.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("keeps a repeated id's turn apart from the live turn that reuses it", () => {
    const store = new FeedStore();
    store.loadHistory({
      kind: "recent",
      messages: [
        message("user", "Earlier", { turn_id: "web-1" }),
        message("assistant", "Earlier reply.", { turn_id: "web-1" }),
      ],
      next_cursor: null,
    });
    store.pushUserMessage("Now", undefined, "web-1");
    store.handleMessage({
      type: "turn_started",
      reply_to: "web-1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "web-1",
      call: 0,
      content: "Looking.",
    });
    const shown = describeEntries(groupTurns(store.feed, store.activeTurnId)).filter(
      (entry) => !entry.startsWith("divider:"),
    );
    expect(shown).toEqual([
      "user:Earlier",
      "turn(assistant:Earlier reply.)",
      "user:Now",
      "turn(assistant:Looking.)",
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
      "turn(assistant:Hi.)",
      "agent-message:Result of the research",
      "turn([memory_add] | assistant:Noted the result.)",
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
      "turn([list_files] | assistant:Two files.)",
      "user:And today?",
      "turn(assistant:Nothing yet.)",
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
      "divider:ep-001",
      "user:Hi",
      "turn(assistant:Hello.)",
      "compressed-marker",
    ]);
  });
});

describe("live turns", () => {
  it("bound a turn by turn_started and turn_ended, and mark it live meanwhile", () => {
    const store = new FeedStore();
    store.pushUserMessage("Check the routing doc");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "Looking first.",
    });
    store.handleMessage({
      type: "tool_call",
      reply_to: "m1",
      call: 0,
      id: "c1",
      name: "memory_search",
      arguments: "{}",
      server: null,
    });

    let entries = groupTurns(store.feed, store.activeTurnId);
    expect(describeEntries(entries)).toEqual([
      "user:Check the routing doc",
      "turn(assistant:Looking first. | [memory_search])",
    ]);
    expect(entries[1]).toMatchObject({ kind: "turn", turnId: "m1", live: true });

    store.handleMessage({
      type: "response",
      reply_to: "m1",
      endpoint: "ws",
      content: "It's tidy.",
    });
    store.handleMessage({ type: "turn_ended", reply_to: "m1" });
    entries = groupTurns(store.feed, store.activeTurnId);
    expect(describeEntries(entries)).toEqual([
      "user:Check the routing doc",
      "turn(assistant:Looking first. | [memory_search] | assistant:It's tidy.)",
    ]);
    expect(entries[1]).toMatchObject({ live: false });
  });

  it("keeps the next turn's tool calls out of the turn before it", () => {
    const store = new FeedStore();
    store.pushUserMessage("First");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "tool_call",
      reply_to: "m1",
      call: 0,
      id: "c1",
      name: "read_file",
      arguments: "{}",
      server: "files",
    });
    store.handleMessage({
      type: "tool_call",
      reply_to: "m1",
      call: 0,
      id: "c1b",
      name: "list_dir",
      arguments: "{}",
      server: "files",
    });
    store.handleMessage({ type: "turn_ended", reply_to: "m1" });
    store.handleMessage({
      type: "turn_started",
      reply_to: "m2",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "tool_call",
      reply_to: "m1",
      call: 0,
      id: "c2",
      name: "write_file",
      arguments: "{}",
      server: "github",
    });

    expect(describeEntries(groupTurns(store.feed, store.activeTurnId))).toEqual([
      "user:First",
      "turn([read_file,list_dir])",
      "turn([write_file])",
    ]);
    expect(store.feed.filter((item) => item.kind === "tool-group")).toMatchObject([
      {
        turnId: "m1",
        calls: [
          { id: "c1", server: "files" },
          { id: "c1b", server: "files" },
        ],
      },
      { turnId: "m2", calls: [{ id: "c2", server: "github" }] },
    ]);
  });

  it("puts a message sent while the turn runs inside it", () => {
    const store = new FeedStore();
    store.pushUserMessage("Draft the post");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "Drafting.",
    });
    store.pushUserMessage("Keep it short");
    store.handleMessage({
      type: "response",
      reply_to: "m1",
      endpoint: "ws",
      content: "Done, and short.",
    });

    expect(describeEntries(groupTurns(store.feed, store.activeTurnId))).toEqual([
      "user:Draft the post",
      "turn(assistant:Drafting. | user:Keep it short | assistant:Done, and short.)",
    ]);
  });

  it("shows the turn in flight after its message before it has output, under one key", () => {
    const store = new FeedStore();
    store.pushUserMessage("Plan the week");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    let entries = groupTurns(store.feed, store.activeTurnId);
    expect(describeEntries(entries)).toEqual(["user:Plan the week", "turn()"]);
    expect(entries[1]).toMatchObject({ key: "turn:m1", live: true });

    store.handleMessage({
      type: "tool_call",
      reply_to: "m1",
      call: 0,
      id: "c1",
      name: "read_file",
      arguments: "{}",
      server: null,
    });
    entries = groupTurns(store.feed, store.activeTurnId);
    expect(entries[1]).toMatchObject({ key: "turn:m1", live: true });
  });

  it("keeps a block for a turn that ended with nothing to show only when asked", () => {
    const store = new FeedStore();
    store.pushUserMessage("Never mind");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
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
      "turn()",
      "user:Something else",
    ]);
  });

  it("marks only the latest block of the turn in flight live", () => {
    const store = new FeedStore();
    store.pushUserMessage("Go");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "First part.",
    });
    store.pushLocalSystem("A note in between.");
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "Second part.",
    });

    const blocks = groupTurns(store.feed, store.activeTurnId).filter((e) => e.kind === "turn");
    expect(blocks.map((b) => b.live)).toEqual([false, true]);
  });

  it("ends a turn the agent stopped in the middle of", () => {
    const store = new FeedStore();
    store.pushUserMessage("Long job");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "Starting.",
    });

    store.abandonLiveTurn();
    expect(store.isProcessing).toBe(false);
    expect(store.activeTurnId).toBeNull();
    expect(groupTurns(store.feed, store.activeTurnId).at(-1)).toMatchObject({ live: false });
  });
});

describe("activity segments", () => {
  it("makes one segment of each run of tool calls between the agent's texts", () => {
    const store = new FeedStore();
    store.pushUserMessage("Fix the port");
    store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "Let me check the config first.",
    });
    for (const id of ["c1", "c2"]) {
      store.handleMessage({
        type: "tool_call",
        reply_to: "m1",
        call: 0,
        id,
        name: "read_file",
        arguments: "{}",
        server: null,
      });
    }
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "The port is set twice.",
    });
    store.handleMessage({
      type: "tool_call",
      reply_to: "m1",
      call: 0,
      id: "c3",
      name: "edit_file",
      arguments: "{}",
      server: null,
    });

    const block = groupTurns(store.feed, store.activeTurnId).at(-1);
    if (block?.kind !== "turn") throw new Error("expected the turn's block");
    expect(
      block.parts.map((part) => (part.kind === "activity" ? part.calls.length : "text")),
    ).toEqual(["text", 2, "text", 1]);
    expect(
      block.parts.flatMap((part) => (part.kind === "activity" ? [part.callsBefore] : [])),
    ).toEqual([0, 2]);
    expect(new Set(block.parts.map((part) => part.key)).size).toBe(4);
  });

  it("joins tool groups that follow each other, as history records one per model call", () => {
    const items = convertHistoryMessages(
      [
        message("user", "Look around", { turn_id: "t1" }),
        ...toolCall("a", "memory_search", "t1"),
        ...toolCall("b", "read_file", "t1"),
        message("assistant", "Found it.", { turn_id: "t1" }),
      ],
      { mode: "main" },
    );
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "user:Look around",
      "turn([memory_search,read_file] | assistant:Found it.)",
    ]);
  });

  it("starts a new segment after an attachment", () => {
    const done = (id: string): ToolCallState => ({
      id,
      name: "exec",
      arguments: {},
      status: "done",
    });
    const items: FeedItem[] = [
      { id: 1, kind: "tool-group", turnId: "t1", calls: [done("a")] },
      {
        id: 2,
        kind: "file-attachment",
        turnId: "t1",
        filename: "chart.png",
        mimeType: "image/png",
        size: 10,
        url: "/files/chart.png",
        caption: null,
      },
      { id: 3, kind: "tool-group", turnId: "t1", calls: [done("b")] },
    ];
    expect(describeEntries(groupTurns(items, null))).toEqual([
      "turn([exec] | file-attachment | [exec])",
    ]);
  });

  it("draws a lead segment for a turn joined at something other than a step", () => {
    const store = new FeedStore();
    store.handleMessage({
      type: "broadcast_response",
      reply_to: "m1",
      call: 0,
      content: "Looking first.",
    });
    const block = groupTurns(store.feed, store.activeTurnId).at(-1);
    if (block?.kind !== "turn") throw new Error("expected the turn's block");

    expect(drawnParts(block, []).map((part) => part.kind)).toEqual(["message"]);
    expect(drawnParts(block, [2]).map((part) => part.kind)).toEqual(["message"]);
    const drawn = drawnParts(block, [0]);
    expect(drawn.map((part) => part.kind)).toEqual(["activity", "message"]);
    expect(drawn[0]).toMatchObject({ calls: [], callsBefore: 0 });
  });

  it("places the notes for missed steps in the segments they fall in", () => {
    const segment = (callsBefore: number, count: number): ActivitySegment => ({
      kind: "activity",
      key: `s${String(callsBefore)}`,
      callsBefore,
      calls: Array.from({ length: count }, (_, i) => ({
        id: `c${String(callsBefore + i)}`,
        name: "exec",
        arguments: {},
        status: "done" as const,
      })),
    });
    const first = segment(0, 2);
    const second = segment(2, 3);
    const gaps = [0, 1, 2, 4, 5];
    // Before everything: the turn's first segment. Past its steps: at the end of the segment they follow.
    expect(gapsWithin(gaps, first, { first: true, last: false })).toEqual([0, 1, 2]);
    expect(gapsWithin(gaps, second, { first: false, last: true })).toEqual([2, 3]);
    // A segment later in the turn takes no note for the head of the turn.
    expect(gapsWithin([0], second, { first: false, last: true })).toEqual([]);
    // A gap past the last step still lands on the last segment.
    expect(gapsWithin([9], second, { first: false, last: true })).toEqual([3]);
    expect(gapsWithin([9], first, { first: true, last: false })).toEqual([]);
  });
});
