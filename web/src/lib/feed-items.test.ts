import { describe, expect, it } from "vitest";
import { convertHistory } from "./feed-items";
import { FeedStore } from "./feed.svelte";
import type { RecentMessage } from "./types";

function message(
  role: RecentMessage["role"],
  content: string,
  extra: Partial<RecentMessage> = {},
): RecentMessage {
  return { role, content, timestamp: "2026-10-09T10:05:00", visibility: "user", ...extra };
}

describe("when converted history says a message was sent", () => {
  const messages = [
    message("user", "Check the wiki"),
    message("assistant", "Looking.", {
      timestamp: "2026-10-09T10:05:30",
      thinking: ["The wiki is where notes live."],
      tool_calls: [{ id: "a", name: "read_file", arguments: {} }],
    }),
    message("tool", "done", { tool_call_id: "a" }),
    message("user", "Result of the research", {
      timestamp: "2026-10-09T10:06:00",
      agent_sender: { address: "spawned-1", category: "spawned" },
    }),
    message("assistant", "Thanks.", { timestamp: "2026-10-09T10:06:10" }),
  ];

  it("stamps what was said with the time history gives it, in the main conversation", () => {
    const { items } = convertHistory(messages, { mode: "main" });
    const stamps = items.map((item) => [item.kind, item.timestamp]);
    expect(stamps).toEqual([
      ["user", "2026-10-09T10:05:00"],
      // The steps between messages are timed live, or not at all.
      ["thinking", undefined],
      ["assistant", "2026-10-09T10:05:30"],
      ["tool-group", undefined],
      ["agent-message", "2026-10-09T10:06:00"],
      ["assistant", "2026-10-09T10:06:10"],
    ]);
  });

  it("stamps nothing in a session's transcript, where every message carries the run's start", () => {
    const { items } = convertHistory(messages, { mode: "session" });
    expect(items.some((item) => item.timestamp !== undefined)).toBe(false);
  });

  it("stamps nothing when told the times aren't the messages' own, as for an archived episode", () => {
    const { items } = convertHistory(messages, { mode: "main", timestamps: false });
    expect(items.some((item) => item.timestamp !== undefined)).toBe(false);
  });

  it("can be told to stamp a session's messages", () => {
    const { items } = convertHistory([message("user", "Go")], {
      mode: "session",
      timestamps: true,
    });
    expect(items[0]).toMatchObject({ kind: "user", timestamp: "2026-10-09T10:05:00" });
  });

  it("leaves a message with no timestamp unstamped", () => {
    const { items } = convertHistory([message("user", "Hi", { timestamp: "" })], { mode: "main" });
    expect(items[0]).not.toHaveProperty("timestamp");
  });
});

describe("a user's message in history", () => {
  const attachment = {
    item_id: "item-1",
    filename: "notes.md",
    size: 12,
    content_type: "text/markdown",
    path: "inbox/agent/attachments/item-1/notes.md",
  };

  it("keeps its images and attachments, so a reload shows them", () => {
    const { items } = convertHistory(
      [
        message("user", "See these", {
          images: [{ media_type: "image/png", data: "AAAA" }],
          attachments: [attachment],
        }),
      ],
      { mode: "main" },
    );
    expect(items[0]).toMatchObject({
      kind: "user",
      content: "See these",
      images: [{ media_type: "image/png", data: "AAAA" }],
      attachments: [attachment],
    });
  });

  it("carries no image or attachment fields when the message had none", () => {
    const { items } = convertHistory([message("user", "Plain")], { mode: "main" });
    expect(items[0]).not.toHaveProperty("images");
    expect(items[0]).not.toHaveProperty("attachments");
  });
});

describe("the times in the store's history", () => {
  it("shows an archived episode's messages with no time, and the recent ones with theirs", () => {
    const store = new FeedStore();
    store.loadHistory({
      kind: "recent",
      messages: [message("user", "Now"), message("assistant", "Here.")],
      next_cursor: "ep-1",
    });
    store.prependEpisode({
      kind: "episode",
      episode_id: "ep-1",
      date: "2026-10-01",
      // An episode records its date as midnight on every message.
      messages: [
        message("user", "Back then", { timestamp: "2026-10-01T00:00:00" }),
        message("assistant", "Yes.", { timestamp: "2026-10-01T00:00:00" }),
      ],
      next_cursor: null,
    });

    const said = store.feed.filter((item) => item.kind === "user" || item.kind === "assistant");
    expect(said.map((item) => [item.content, item.timestamp])).toEqual([
      ["Back then", undefined],
      ["Yes.", undefined],
      ["Now", "2026-10-09T10:05:00"],
      ["Here.", "2026-10-09T10:05:00"],
    ]);
  });

  it("stamps the recent messages that an older episode decides to show, and not the episode's", () => {
    const store = new FeedStore();
    // The recent history begins in the middle of a background turn that older history began.
    store.loadHistory({
      kind: "recent",
      messages: [
        message("assistant", "Carrying on.", { visibility: "background" }),
        message("user", "Now"),
      ],
      next_cursor: "ep-1",
    });
    expect(store.feed.map((item) => item.kind)).toEqual(["user"]);

    // The episode ends in a message from an agent, which makes the turn one the user sees.
    store.prependEpisode({
      kind: "episode",
      episode_id: "ep-1",
      date: "2026-10-01",
      messages: [
        message("user", "Result", {
          timestamp: "2026-10-01T00:00:00",
          visibility: "background",
          agent_sender: { address: "spawned-1", category: "spawned" },
        }),
      ],
      next_cursor: null,
    });

    const carried = store.feed.find((item) => item.kind === "assistant");
    expect(carried).toMatchObject({ content: "Carrying on.", timestamp: "2026-10-09T10:05:00" });
    const episodes = store.feed.find((item) => item.kind === "agent-message");
    expect(episodes).not.toHaveProperty("timestamp");
  });
});

describe("a session's message to main in history", () => {
  it("keeps the run that sent it, so Open session opens that run", () => {
    const { items } = convertHistory(
      [
        message("user", "[Agent Message from spawned-1 (spawned)]\nResult", {
          visibility: "background",
          agent_sender: { address: "spawned-1", category: "spawned", run_id: "run-4" },
        }),
        message("user", "[Agent Message from spawned-2 (spawned)]\nOlder result", {
          visibility: "background",
          agent_sender: { address: "spawned-2", category: "spawned" },
        }),
      ],
      { mode: "main" },
    );

    expect(items).toMatchObject([
      { kind: "agent-message", from: "spawned-1", content: "Result", runId: "run-4" },
      { kind: "agent-message", from: "spawned-2", content: "Older result", runId: null },
    ]);
  });
});

describe("context a session reads in its transcript", () => {
  const pointer =
    "[Resumed session] Your previous run (run-1) was merged as episode ep-1 — retrieve it with memory_get if useful.";

  it("is left out, while the messages it opens are drawn", () => {
    const { items } = convertHistory(
      [
        message("user", pointer, { context: true }),
        message(
          "user",
          "[Message from the owner via the web UI — your response in this turn is shown to them directly]\nPick it back up",
          { id: "m-1" },
        ),
        message("assistant", "On it."),
      ],
      { mode: "session" },
    );

    expect(items).toMatchObject([
      { kind: "user", content: "Pick it back up" },
      { kind: "assistant", content: "On it." },
    ]);
    expect(items).toHaveLength(2);
  });

  it("is drawn when the transcript doesn't mark it, as an ordinary user entry", () => {
    const { items } = convertHistory([message("user", pointer)], { mode: "session" });
    expect(items).toMatchObject([{ kind: "user", content: pointer }]);
  });

  it("leaves a chat message read mid-turn where the session read it", () => {
    const { items } = convertHistory(
      [
        message("assistant", "Checking.", {
          tool_calls: [{ id: "a", name: "read_file", arguments: {} }],
        }),
        message("tool", "done", { tool_call_id: "a" }),
        message("system", "[14:00] Sam: build is red", { context: true }),
        message("user", "is it green yet?", {
          sender: { name: "Jane", id: "j1", interface: "discord" },
        }),
      ],
      { mode: "session" },
    );

    const chat = items.find((item) => item.kind === "user");
    expect(chat).toMatchObject({ content: "is it green yet?", midTurn: true });
  });
});
