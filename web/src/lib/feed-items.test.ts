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
