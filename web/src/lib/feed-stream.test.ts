import { describe, expect, it, vi } from "vitest";
import { LiveStreams, MessageIds } from "./feed-stream";
import type { AssistantFeedItem, FeedItem, ThinkingFeedItem } from "./types";

/** Streams over a plain feed, with frames that run when the test says so. */
function setup(): { feed: FeedItem[]; streams: LiveStreams; frame: () => void; frames: number } {
  const feed: FeedItem[] = [];
  const queued: Array<() => void> = [];
  const result = {
    feed,
    streams: new LiveStreams(feed, (flush) => queued.push(flush)),
    frame: () => {
      for (const flush of queued.splice(0)) flush();
    },
    get frames() {
      return queued.length;
    },
  };
  return result;
}

const tag = { turnId: "t1" };

function assistants(feed: FeedItem[]): AssistantFeedItem[] {
  return feed.filter((item): item is AssistantFeedItem => item.kind === "assistant");
}

function thoughts(feed: FeedItem[]): ThinkingFeedItem[] {
  return feed.filter((item): item is ThinkingFeedItem => item.kind === "thinking");
}

describe("streamed text", () => {
  it("makes a streaming draft of the first piece, tied to the call that writes it", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "Let me", tag);
    expect(feed).toMatchObject([
      { kind: "assistant", content: "Let me", call: 0, streaming: true, turnId: "t1" },
    ]);
  });

  it("holds later pieces until the next frame, then adds them all at once", () => {
    const { feed, streams, frame } = setup();
    streams.appendText("t1", 0, "Let", tag);
    streams.appendText("t1", 0, " me", tag);
    streams.appendText("t1", 0, " look", tag);
    expect(assistants(feed)[0]?.content).toBe("Let");

    frame();
    expect(assistants(feed)[0]?.content).toBe("Let me look");
  });

  it("asks for one frame however many pieces arrive before it", () => {
    const run = setup();
    run.streams.appendText("t1", 0, "a", tag);
    run.streams.appendText("t1", 0, "b", tag);
    run.streams.appendText("t1", 0, "c", tag);
    run.streams.appendThought("t1", 0, "d", tag);
    run.streams.appendThought("t1", 0, "e", tag);
    expect(run.frames).toBe(1);

    run.frame();
    run.streams.appendText("t1", 0, "f", tag);
    expect(run.frames).toBe(1);
  });

  it("keeps each model call's text apart", () => {
    const { feed, streams, frame } = setup();
    streams.appendText("t1", 0, "first", tag);
    streams.appendText("t1", 1, "second", tag);
    streams.appendText("t1", 0, " more", tag);
    streams.appendText("t1", 1, " more", tag);
    frame();
    expect(assistants(feed).map((item) => item.content)).toEqual(["first more", "second more"]);
  });

  it("is replaced by the call's authoritative text, in the same item", () => {
    const { feed, streams, frame } = setup();
    streams.appendText("t1", 0, "Let me", tag);
    const before = feed[0]?.id;
    streams.appendText("t1", 0, " look", tag);
    streams.completeText("t1", 0, "Let me look at the config.", tag);
    frame();
    expect(feed).toHaveLength(1);
    expect(feed[0]).toMatchObject({
      id: before,
      content: "Let me look at the config.",
      streaming: false,
    });
  });

  it("adds the authoritative text as a new item when nothing streamed", () => {
    const { feed, streams } = setup();
    streams.completeText("t1", 0, "Looking.", tag);
    expect(feed).toMatchObject([{ kind: "assistant", content: "Looking.", call: 0, turnId: "t1" }]);
    expect(feed[0]).not.toHaveProperty("streaming");
  });

  it("adds text of no model call as it is, with where it was delivered", () => {
    const { feed, streams } = setup();
    streams.completeText("t1", undefined, "Posted.", tag, "telegram");
    expect(feed[0]).toMatchObject({ content: "Posted.", deliveredTo: "telegram" });
    expect(feed[0]).not.toHaveProperty("call");
  });

  it("notes where a streamed reply was delivered when its authoritative text arrives", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 2, "Done", tag);
    streams.completeText("t1", 2, "Done.", tag, "telegram");
    expect(feed[0]).toMatchObject({ content: "Done.", deliveredTo: "telegram", streaming: false });
  });

  it("drops a draft the authoritative text says was empty", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "hm", tag);
    streams.completeText("t1", 0, "", tag);
    expect(feed).toEqual([]);
    streams.completeText("t1", 1, "", tag);
    expect(feed).toEqual([]);
  });

  it("doesn't add held pieces to text the authoritative message already replaced", () => {
    const { feed, streams, frame } = setup();
    streams.appendText("t1", 0, "Let", tag);
    streams.appendText("t1", 0, " me", tag);
    streams.completeText("t1", 0, "Let me look.", tag);
    frame();
    expect(assistants(feed)[0]?.content).toBe("Let me look.");
  });

  it("starts a draft over when its call does", () => {
    const { feed, streams, frame } = setup();
    streams.appendText("t1", 0, "Let me", tag);
    streams.appendText("t1", 0, " look", tag);
    streams.restart("t1", 0);
    frame();
    expect(feed).toEqual([]);

    streams.appendText("t1", 0, "I'll check", tag);
    expect(assistants(feed).map((item) => item.content)).toEqual(["I'll check"]);
  });
});

describe("a turn ending with text unfinished", () => {
  it("keeps what arrived, and says a stop cut it short", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "Let", tag);
    streams.appendText("t1", 0, " me", tag);
    streams.finish("t1", "stopped");
    expect(feed[0]).toMatchObject({ content: "Let me", streaming: false, cut: "stopped" });
  });

  it("says when the agent stopped under it", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "Let", tag);
    streams.finish("t1", "interrupted");
    expect(feed[0]).toMatchObject({ cut: "interrupted" });
  });

  it("leaves text of a turn that finished on its own unmarked", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "Done.", tag);
    streams.finish("t1", "finished");
    expect(feed[0]).toMatchObject({ streaming: false });
    expect(feed[0]).not.toHaveProperty("cut");
  });

  it("leaves another turn's drafts alone", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "one", tag);
    streams.appendText("t2", 0, "two", { turnId: "t2" });
    streams.finish("t1", "stopped");
    expect(feed[0]).toMatchObject({ streaming: false });
    expect(feed[1]).toMatchObject({ streaming: true });
  });

  it("forgets its drafts, so a late authoritative message adds a new item", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "one", tag);
    streams.finish("t1", "finished");
    streams.completeText("t1", 0, "one, in full", tag);
    expect(assistants(feed).map((item) => item.content)).toEqual(["one", "one, in full"]);
  });
});

describe("streamed reasoning", () => {
  it("shows as it arrives, timed from its first piece", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(10_000);
      const { feed, streams, frame } = setup();
      streams.appendThought("t1", 0, "The user wants", tag);
      streams.appendThought("t1", 0, " the config.", tag);
      frame();
      expect(thoughts(feed)[0]).toMatchObject({
        content: "The user wants the config.",
        streaming: true,
        startedAt: 10_000,
        call: 0,
      });

      vi.setSystemTime(16_000);
      streams.completeThought("t1", 0, "The user wants the config file.", tag);
      expect(thoughts(feed)[0]).toMatchObject({
        content: "The user wants the config file.",
        streaming: false,
        startedAt: 10_000,
        endedAt: 16_000,
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("ends when the call's text begins, and the authoritative reasoning then replaces it", () => {
    const { feed, streams } = setup();
    streams.appendThought("t1", 0, "hm", tag);
    streams.endThought("t1", 0);
    expect(thoughts(feed)[0]).toMatchObject({ streaming: false });
    expect(thoughts(feed)[0]?.endedAt).toBeDefined();

    streams.completeThought("t1", 0, "hm, the whole of it", tag);
    expect(thoughts(feed)).toHaveLength(1);
    expect(thoughts(feed)[0]?.content).toBe("hm, the whole of it");
  });

  it("is added whole, untimed, when nothing streamed", () => {
    const { feed, streams } = setup();
    streams.completeThought("t1", 0, "Checking the index first.", tag);
    expect(feed).toMatchObject([{ kind: "thinking", content: "Checking the index first." }]);
    expect(feed[0]).not.toHaveProperty("startedAt");
  });

  it("is left out when it holds nothing readable", () => {
    const { feed, streams } = setup();
    streams.completeThought("t1", 0, "  ", tag);
    expect(feed).toEqual([]);
  });

  it("goes back before the text the call had already streamed", () => {
    const { feed, streams } = setup();
    feed.push({ id: 1, kind: "user", content: "Fix the port", turnId: "t1" });
    streams.appendText("t1", 0, "Let me check", tag);
    streams.completeThought("t1", 0, "The port is probably set twice.", tag);
    expect(feed.map((item) => item.kind)).toEqual(["user", "thinking", "assistant"]);
  });

  it("goes after whatever an earlier call left, and before this call's text", () => {
    const { feed, streams } = setup();
    streams.appendText("t1", 0, "first", tag);
    streams.completeText("t1", 0, "first.", tag);
    streams.appendText("t1", 1, "second", tag);
    streams.completeThought("t1", 1, "On the second call.", tag);
    expect(feed.map((item) => (item.kind === "thinking" ? "thinking" : item.kind))).toEqual([
      "assistant",
      "thinking",
      "assistant",
    ]);
    expect(assistants(feed).map((item) => item.content)).toEqual(["first.", "second"]);
  });

  it("is dropped with a call that starts over", () => {
    const { feed, streams } = setup();
    streams.appendThought("t1", 0, "hm", tag);
    streams.appendText("t1", 0, "Let", tag);
    streams.restart("t1", 0);
    expect(feed).toEqual([]);
  });

  it("ends with its turn", () => {
    const { feed, streams } = setup();
    streams.appendThought("t1", 0, "hm", tag);
    streams.finish("t1", "stopped");
    expect(thoughts(feed)[0]).toMatchObject({ streaming: false });
    expect(thoughts(feed)[0]?.endedAt).toBeDefined();
  });
});

describe("MessageIds", () => {
  it("remembers the ids it was given", () => {
    const ids = new MessageIds();
    ids.add("web-1");
    expect(ids.has("web-1")).toBe(true);
    expect(ids.has("web-2")).toBe(false);
  });
});
