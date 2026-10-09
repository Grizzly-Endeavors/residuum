import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServerMessage } from "../src/lib/generated/protocol";
import type { ChatHistorySegment, RecentMessage } from "../src/lib/types";
import { chatHistorySegment, chatRoutes, createChatSimulator, type ChatSimulator } from "./chat";
import { deltaFrames, PIECE_MS } from "./chat-scenarios";
import { cannedResponses } from "./data/chat";
import { createState, type MockHub, type MockState } from "./state";
import {
  captureFrames,
  createStubHub,
  fetchJson,
  startRouteHarness,
  type RouteHarness,
} from "./test-support";

describe("chat history", () => {
  it("serves the recent messages first, then walks the episodes back to the oldest", () => {
    const state = createState("atlas");
    const recent = chatHistorySegment(state, null);
    expect(recent).toMatchObject({ kind: "recent", next_cursor: "ep-003" });
    expect(recent?.messages.length).toBeGreaterThan(5);

    const chain: string[] = [];
    let cursor = recent?.next_cursor ?? null;
    while (cursor !== null) {
      const segment = chatHistorySegment(state, cursor);
      expect(segment).toMatchObject({ kind: "episode", episode_id: cursor });
      chain.push(cursor);
      cursor = segment?.next_cursor ?? null;
    }
    expect(chain).toEqual(["ep-003", "ep-002", "ep-001"]);
  });

  it("has no episode for an unknown cursor", () => {
    expect(chatHistorySegment(createState("atlas"), "ep-999")).toBeNull();
  });

  it("puts the messages recorded since the sample history after it", () => {
    const state = createState("atlas");
    const sampleCount = chatHistorySegment(state, null)?.messages.length ?? 0;
    state.extraRecent.push({
      role: "user",
      content: "newest",
      timestamp: new Date().toISOString(),
      visibility: "user",
    });
    const messages = chatHistorySegment(state, null)?.messages ?? [];
    expect(messages).toHaveLength(sampleCount + 1);
    expect(messages.at(-1)?.content).toBe("newest");
  });

  it("carries tool call arguments as an object, like the backend's serde_json value", () => {
    const messages = chatHistorySegment(createState("atlas"), null)?.messages ?? [];
    const calls = messages.flatMap((m) => m.tool_calls ?? []);
    expect(calls.length).toBeGreaterThan(0);
    for (const call of calls) expect(typeof call.arguments).toBe("object");
  });

  it("has no conversation for an agent that never ran", () => {
    const state = createState("drifter", false);
    expect(chatHistorySegment(state, null)).toEqual({
      kind: "recent",
      messages: [],
      next_cursor: null,
    });
    expect(chatHistorySegment(state, "ep-003")).toBeNull();
  });

  it("moves the recorded messages into a new episode once history is compressed", () => {
    const state = createState("atlas");
    const push = (content: string): number =>
      state.extraRecent.push({
        role: "user",
        content,
        timestamp: new Date().toISOString(),
        visibility: "user",
      });
    push("before");
    state.compressedAt = state.extraRecent.length;
    push("after");

    expect(chatHistorySegment(state, null)).toMatchObject({
      kind: "recent",
      next_cursor: "ep-004",
      messages: [{ content: "after" }],
    });
    const episode = chatHistorySegment(state, "ep-004");
    expect(episode).toMatchObject({ kind: "episode", episode_id: "ep-004", next_cursor: "ep-003" });
    expect(episode?.messages.map((m) => m.content)).toContain("before");
    expect(episode?.messages.every((m) => m.visibility === "user")).toBe(true);
  });
});

describe("chat routes", () => {
  let harness: RouteHarness;

  beforeAll(async () => {
    harness = await startRouteHarness(chatRoutes);
  });

  afterAll(async () => {
    await harness.close();
  });

  it("answers the history and an episode of it", async () => {
    const recent = await fetchJson(`${harness.baseUrl}/api/chat/history`);
    expect(recent.status).toBe(200);
    expect(recent.body).toMatchObject({ kind: "recent", next_cursor: "ep-003" });
    const episode = await fetchJson(`${harness.baseUrl}/api/chat/history?episode=ep-002`);
    expect(episode.body).toMatchObject({ kind: "episode", episode_id: "ep-002" });
  });

  it("answers 404 for an episode that doesn't exist", async () => {
    expect(await fetchJson(`${harness.baseUrl}/api/chat/history?episode=nope`)).toEqual({
      status: 404,
      body: { error: "episode not found" },
    });
  });

  it("reports the conversation's usage totals", async () => {
    expect(await fetchJson(`${harness.baseUrl}/api/usage`)).toEqual({
      status: 200,
      body: { input_tokens: 412_880, output_tokens: 9_214, context_tokens: 18_402, tool_calls: 37 },
    });
  });
});

describe("chat turns", () => {
  /** When a plain turn is let end, and how long its last reply takes to stream in after that. */
  const END_MS = 1500;
  const streamMs = (reply: string): number =>
    deltaFrames("m1", "text", 2, reply).length * PIECE_MS + 40;
  const FIRST_TURN_MS = END_MS + streamMs(cannedResponses[0] ?? "");
  const TURN_MS = END_MS + Math.max(...cannedResponses.map(streamMs));
  const DROP_MS = 600;
  const RECONNECT_MS = 3500;
  const DROP_TURN_MS = 4000;
  const DROP_FINISH_MS = 900;

  let hub: MockHub;
  let frames: ServerMessage[];
  let chat: ChatSimulator;
  let drops: number;
  let busy: boolean[];
  let unread: number;
  let connected: number;
  let state: MockState;

  function message(
    content: string,
    id = "m1",
  ): { type: "send_message"; id: string; content: string } {
    return { type: "send_message", id, content };
  }

  /** The kinds of frame sent, leaving out the pieces text and reasoning stream in as. */
  function types(): string[] {
    return frames
      .filter((f) => f.type !== "text_delta" && f.type !== "thinking_delta")
      .map((f) => f.type);
  }

  function recorded(): RecentMessage[] {
    return state.extraRecent;
  }

  beforeEach(() => {
    vi.useFakeTimers();
    hub = createStubHub();
    const agent = hub.createAgent("atlas");
    ({ state } = agent);
    frames = captureFrames(state);
    drops = 0;
    state.dropSockets = () => {
      drops++;
    };
    busy = [];
    unread = 0;
    connected = 1;
    agent.connectedClients = () => connected;
    hub.setBusy = (_agent, value) => {
      busy.push(value);
    };
    hub.addUnread = () => {
      unread++;
    };
    chat = createChatSimulator(hub, agent);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("runs a turn: started, a note, a search, two reads, the reply, ended", () => {
    chat.send(message("hello there"));
    expect(types()).toEqual(["user_message", "turn_started"]);
    expect(frames[0]).toEqual({
      type: "user_message",
      id: "m1",
      turn_id: "m1",
      content: "hello there",
      endpoint: "ws",
    });
    expect(frames[1]).toEqual({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });

    vi.advanceTimersByTime(300);
    expect(types()).toEqual([
      "user_message",
      "turn_started",
      "thinking",
      "broadcast_response",
      "tool_call",
    ]);

    vi.advanceTimersByTime(FIRST_TURN_MS - 300);
    expect(types()).toEqual([
      "user_message",
      "turn_started",
      "thinking",
      "broadcast_response",
      "tool_call",
      "tool_result",
      "turn_usage",
      "tool_call",
      "tool_call",
      "tool_result",
      "tool_result",
      "turn_usage",
      "response",
      "turn_ended",
    ]);
    const call = frames.find((f) => f.type === "tool_call");
    const result = frames.find((f) => f.type === "tool_result");
    const response = frames.find((f) => f.type === "response");
    expect(call).toMatchObject({
      type: "tool_call",
      name: "memory_search",
      arguments: { query: "hello there", limit: 5 },
      server: null,
    });
    expect(result).toMatchObject({
      type: "tool_result",
      name: "memory_search",
      is_error: false,
    });
    expect(response).toEqual({
      type: "response",
      reply_to: "m1",
      call: 2,
      endpoint: "ws",
      content: cannedResponses[0],
    });
    const reads = frames.filter((f) => f.type === "tool_call" && f.name === "read_file");
    expect(reads.map((f) => f.type === "tool_call" && f.arguments)).toEqual([
      { path: "team/wiki/index.md" },
      { path: "team/wiki/projects/residuum.md" },
    ]);
  });

  it("fails the second read for a message starting with fail", () => {
    chat.send(message("fail to read it"));
    vi.advanceTimersByTime(TURN_MS);
    const failed = frames.flatMap((f) => (f.type === "tool_result" ? [f.is_error] : []));
    expect(failed).toEqual([false, false, true]);
  });

  it("sends the tool call's arguments as an object, matching the generated protocol", () => {
    chat.send(message("x".repeat(300)));
    vi.advanceTimersByTime(300);
    const call = frames.find((f) => f.type === "tool_call");
    expect(call).toMatchObject({ arguments: { query: "x".repeat(100), limit: 5 } });
  });

  it("cycles through the canned replies", () => {
    for (let i = 0; i <= cannedResponses.length; i++) {
      chat.send(message("again", `m${i}`));
      vi.advanceTimersByTime(END_MS + streamMs(cannedResponses[i % cannedResponses.length] ?? ""));
    }
    const replies = frames.flatMap((f) => (f.type === "response" ? [f.content] : []));
    expect(replies).toEqual([...cannedResponses, cannedResponses[0]]);
  });

  it("records the whole turn in history when it ends", () => {
    chat.send(message("hello there"));
    vi.advanceTimersByTime(FIRST_TURN_MS - 1);
    expect(recorded()).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(recorded().map((m) => m.role)).toEqual([
      "user",
      "assistant",
      "tool",
      "assistant",
      "tool",
      "tool",
      "assistant",
    ]);
    expect(recorded()[0]?.content).toBe("hello there");
    // What the model reasoned goes with the call that did it.
    expect(recorded()[1]?.thinking).toHaveLength(1);
    expect(recorded()[1]?.tool_calls?.[0]).toMatchObject({ name: "memory_search", server: null });
    expect(recorded()[3]?.tool_calls).toMatchObject([
      { name: "read_file", server: null },
      { name: "read_file", server: null },
    ]);
    expect(recorded()[6]?.content).toBe(cannedResponses[0]);
    // Tagged with the turn's correlation id, so a page that saw it live can tell it's recorded.
    expect(new Set(recorded().map((m) => m.turn_id))).toEqual(new Set(["m1"]));
  });

  it("adds each turn's usage to the conversation's totals, and reports them", () => {
    const before = { ...state.usage };
    chat.send(message("hello"));
    vi.advanceTimersByTime(TURN_MS);
    expect(state.usage.input_tokens).toBeGreaterThan(before.input_tokens);
    expect(state.usage.output_tokens).toBeGreaterThan(before.output_tokens);
    expect(state.usage.tool_calls).toBe(before.tool_calls + 3);
    expect(frames.filter((f) => f.type === "turn_usage").at(-1)).toMatchObject({
      reply_to: "m1",
      has_usage: true,
      tool_calls: 3,
      session_totals: state.usage,
    });
  });

  it("follows a message starting with remember with memory work, then ends it", () => {
    const work = (): unknown[] => frames.filter((f) => f.type === "post_turn_activity");
    chat.send(message("hello"));
    vi.advanceTimersByTime(TURN_MS);
    expect(work()).toEqual([]);

    chat.send(message("remember the plants", "m2"));
    vi.advanceTimersByTime(TURN_MS);
    expect(work()).toEqual([{ type: "post_turn_activity", kind: "memory", active: true }]);
    vi.advanceTimersByTime(2000);
    expect(work()).toEqual([
      { type: "post_turn_activity", kind: "memory", active: true },
      { type: "post_turn_activity", kind: "memory", active: false },
    ]);
  });

  it("marks the agent busy for the length of the turn", () => {
    chat.send(message("hello"));
    expect(busy).toEqual([true]);
    vi.advanceTimersByTime(TURN_MS);
    expect(busy).toEqual([true, false]);
  });

  it("counts the reply unread only when no page has the agent open", () => {
    chat.send(message("seen"));
    vi.advanceTimersByTime(TURN_MS);
    expect(unread).toBe(0);
    connected = 0;
    chat.send(message("missed", "m2"));
    vi.advanceTimersByTime(TURN_MS);
    expect(unread).toBe(1);
  });

  describe("drop", () => {
    it("loses the connection mid-turn, then finishes live after the page is back", () => {
      chat.send(message("drop please"));
      vi.advanceTimersByTime(DROP_MS);
      expect(drops).toBe(1);
      // The search and the reads it started went out first; the reads' results are lost.
      expect(types()).toEqual([
        "user_message",
        "turn_started",
        "thinking",
        "broadcast_response",
        "tool_call",
        "tool_result",
        "turn_usage",
        "tool_call",
        "tool_call",
      ]);

      // Frames sent while the connection is down go nowhere.
      vi.advanceTimersByTime(RECONNECT_MS - DROP_MS);
      expect(types()).toHaveLength(9);

      vi.advanceTimersByTime(DROP_TURN_MS - RECONNECT_MS + streamMs(cannedResponses[0] ?? ""));
      expect(types().slice(9)).toEqual(["turn_usage", "response", "turn_ended"]);
      expect(busy).toEqual([true, false]);
      expect(recorded()).toHaveLength(7);
    });

    it("drop finish: the turn ends while the page is away, so only history has it", () => {
      chat.send(message("drop finish now"));
      vi.advanceTimersByTime(DROP_FINISH_MS);
      expect(drops).toBe(1);
      expect(types()).toEqual([
        "user_message",
        "turn_started",
        "thinking",
        "broadcast_response",
        "tool_call",
        "tool_result",
        "turn_usage",
        "tool_call",
        "tool_call",
      ]);
      expect(busy).toEqual([true, false]);
      expect(recorded().map((m) => m.role)).toEqual([
        "user",
        "assistant",
        "tool",
        "assistant",
        "tool",
        "tool",
        "assistant",
      ]);

      // The page never comes back in the simulation's timeline: nothing more is sent.
      vi.advanceTimersByTime(DROP_TURN_MS);
      expect(types()).toHaveLength(9);
    });

    it("drop compress: history is compressed into ep-004 while the page is away", () => {
      state.extraRecent.push({
        role: "user",
        content: "earlier",
        timestamp: new Date().toISOString(),
        visibility: "user",
      });
      chat.send(message("drop compress it"));
      vi.advanceTimersByTime(DROP_MS);
      expect(state.compressedAt).toBe(1);
      expect(drops).toBe(1);

      vi.advanceTimersByTime(DROP_TURN_MS + TURN_MS);
      const recent = chatHistorySegment(state, null) as ChatHistorySegment;
      expect(recent).toMatchObject({ kind: "recent", next_cursor: "ep-004" });
      // The turn that finished after the compression is the recent history.
      expect(recent.messages.map((m) => m.content)).toContain("drop compress it");
      const episode = chatHistorySegment(state, "ep-004");
      expect(episode?.messages.map((m) => m.content)).toContain("earlier");
    });
  });

  describe("scripted turns", () => {
    /** Long enough for any scripted turn to run to its end. */
    const SCRIPT_MS = 8000;

    /** The frames a turn sent in order, a run of pieces of one stream as one entry. */
    function outline(): string[] {
      const labels = frames.map((f) => {
        if (f.type === "text_delta") return `stream:${String(f.call)}`;
        if (f.type === "thinking_delta") return `think-stream:${String(f.call)}`;
        if (f.type === "broadcast_response") return `text:${f.content.split(" ")[0] ?? ""}`;
        if (f.type === "tool_call") return `call:${f.name}`;
        if (f.type === "tool_result") return `result:${f.name}`;
        return f.type;
      });
      return labels.filter((label, i) => !label.includes("stream:") || label !== labels[i - 1]);
    }

    it("segments: thinks and reads, then works through two more rounds, each after a text", () => {
      chat.send(message("segments please"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(outline()).toEqual([
        "user_message",
        "turn_started",
        "think-stream:0",
        "thinking",
        "call:read_file",
        "call:read_file",
        "call:read_file",
        "result:read_file",
        "result:read_file",
        "result:read_file",
        "stream:1",
        "text:Let",
        "call:exec",
        "call:exec",
        "result:exec",
        "result:exec",
        "stream:2",
        "text:The",
        "call:edit_file",
        "result:edit_file",
        "think-stream:3",
        "thinking",
        "stream:3",
        "turn_usage",
        "response",
        "turn_ended",
      ]);
      expect(frames.find((f) => f.type === "turn_usage")).toMatchObject({ tool_calls: 6 });
    });

    it("segments: records every round in history as the agent made it", () => {
      chat.send(message("segments please"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(recorded().map((m) => m.role)).toEqual([
        "user",
        "assistant",
        "tool",
        "tool",
        "tool",
        "assistant",
        "tool",
        "tool",
        "assistant",
        "tool",
        "assistant",
      ]);
      expect(recorded()[1]).toMatchObject({
        content: "",
        thinking: [expect.stringContaining("port clash") as string],
        tool_calls: [{ name: "read_file" }, { name: "read_file" }, { name: "read_file" }],
      });
      expect(recorded()[5]).toMatchObject({ content: "Let me check the config first." });
      expect(recorded().at(-1)).toMatchObject({
        thinking: [expect.stringContaining("restart") as string],
      });
      expect(recorded().at(-1)?.tool_calls).toBeUndefined();
      expect(new Set(recorded().map((m) => m.turn_id))).toEqual(new Set(["m1"]));
      expect(state.usage.tool_calls).toBeGreaterThanOrEqual(6);
    });

    it("segments: waits with the last round's edit done while turns are held", () => {
      hub.env.holdTurns("end");
      chat.send(message("segments please"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(outline().at(-1)).toBe("result:edit_file");
      expect(types()).not.toContain("turn_ended");
      hub.env.holdTurns("none");
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(types().at(-1)).toBe("turn_ended");
    });

    it("error: writes its note, searches, then fails with a plain message and its cause", () => {
      chat.send(message("error please"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(outline()).toEqual([
        "user_message",
        "turn_started",
        "stream:0",
        "text:Looking",
        "call:memory_search",
        "result:memory_search",
        "turn_usage",
        "error",
        "turn_ended",
      ]);
      expect(frames.find((f) => f.type === "error")).toMatchObject({
        reply_to: "m1",
        message: expect.stringContaining("didn't answer") as string,
        details: expect.stringContaining("503") as string,
      });
    });

    it("error: keeps only the user's message, and doesn't count a reply unread", () => {
      connected = 0;
      chat.send(message("error please"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(recorded().map((m) => m.content)).toEqual(["error please"]);
      expect(unread).toBe(0);
      expect(busy).toEqual([true, false]);
    });

    it("retry: streams part of a reply, starts over, and ends with the second attempt", () => {
      chat.send(message("retry please"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(outline()).toEqual([
        "user_message",
        "turn_started",
        "stream:0",
        "stream_restart",
        "stream:0",
        "turn_usage",
        "response",
        "turn_ended",
      ]);
      expect(frames.find((f) => f.type === "stream_restart")).toEqual({
        type: "stream_restart",
        reply_to: "m1",
        call: 0,
      });
      expect(recorded().at(-1)?.content).toContain("routing doc sends urgent notices");
    });

    it("think: thinks at length, then gives a short answer", () => {
      chat.send(message("think about it"));
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(outline()).toEqual([
        "user_message",
        "turn_started",
        "think-stream:0",
        "thinking",
        "stream:0",
        "turn_usage",
        "response",
        "turn_ended",
      ]);
      const thought = frames.find((f) => f.type === "thinking");
      expect(thought?.type === "thinking" ? thought.content.split("\n") : []).toHaveLength(4);
    });

    it("streams text in pieces that add up to the complete message", () => {
      chat.send(message("think about it"));
      vi.advanceTimersByTime(SCRIPT_MS);
      const streamed = frames.flatMap((f) => (f.type === "text_delta" ? [f.text] : [])).join("");
      const response = frames.find((f) => f.type === "response");
      expect(response?.type === "response" ? response.content : "").toBe(streamed);
      expect(frames.filter((f) => f.type === "text_delta").length).toBeGreaterThan(1);
    });

    it("can be stopped like any turn, keeping what streamed", () => {
      chat.send(message("segments please"));
      vi.advanceTimersByTime(500);
      chat.cancel("m1");
      expect(frames.at(-1)).toEqual({ type: "turn_ended", reply_to: "m1" });
      vi.advanceTimersByTime(SCRIPT_MS);
      expect(types().filter((t) => t === "turn_ended")).toHaveLength(1);
      expect(types()).not.toContain("response");
      expect(frames.some((f) => f.type === "thinking_delta")).toBe(true);
    });
  });

  describe("a message from another channel", () => {
    const alex = {
      name: "Alex",
      id: "42",
      interface: "telegram",
      location: "direct message",
    };

    it("reaches every page as the person's message, then starts a turn from that channel", () => {
      chat.receive("Can you check the routing doc?", { endpoint: "telegram", sender: alex });
      expect(frames[0]).toEqual({
        type: "user_message",
        id: "telegram-1",
        turn_id: "telegram-1",
        content: "Can you check the routing doc?",
        sender: alex,
        endpoint: "telegram",
      });
      expect(frames[1]).toEqual({
        type: "turn_started",
        reply_to: "telegram-1",
        origin: { endpoint: "telegram", sender: alex, visibility: "user" },
      });
    });

    it("answers on that channel, and records the message with who sent it", () => {
      chat.receive("Can you check the routing doc?", { endpoint: "telegram", sender: alex });
      vi.advanceTimersByTime(8000);
      expect(frames.find((f) => f.type === "response")).toMatchObject({
        reply_to: "telegram-1",
        endpoint: "telegram",
      });
      expect(frames.at(-1)).toEqual({ type: "turn_ended", reply_to: "telegram-1" });
      expect(recorded()[0]).toMatchObject({
        role: "user",
        content: "Can you check the routing doc?",
        sender: alex,
        turn_id: "telegram-1",
      });
      expect(unread).toBe(0);
    });

    it("counts the reply unread when no page has the agent open", () => {
      connected = 0;
      chat.receive("Hello?", { endpoint: "telegram", sender: alex });
      vi.advanceTimersByTime(8000);
      expect(unread).toBe(1);
    });

    it("can be stopped from the page, keeping the sender on what is recorded", () => {
      chat.receive("Hello?", { endpoint: "telegram", sender: alex });
      vi.advanceTimersByTime(100);
      chat.cancel("telegram-1");
      expect(recorded()).toMatchObject([{ role: "user", sender: alex }]);
    });
  });

  describe("images sent with a message", () => {
    it("are in the message every page is told of", () => {
      const images = [{ media_type: "image/png", data: "AAAA" }];
      chat.send({ type: "send_message", id: "m1", content: "What is this?", images });
      expect(frames[0]).toMatchObject({ type: "user_message", images });
    });
  });

  describe("cancel", () => {
    it("ends a running turn early, with no reply", () => {
      chat.send(message("stop me"));
      vi.advanceTimersByTime(500);
      chat.cancel("m1");
      expect(types().at(-1)).toBe("turn_ended");
      expect(frames.at(-1)).toEqual({ type: "turn_ended", reply_to: "m1" });
      expect(busy).toEqual([true, false]);
      // Only the user's message is kept; the reply never comes.
      expect(recorded().map((m) => m.content)).toEqual(["stop me"]);

      vi.advanceTimersByTime(TURN_MS);
      expect(types().filter((t) => t === "response")).toEqual([]);
      expect(types().filter((t) => t === "turn_ended")).toHaveLength(1);
    });

    it("ignores a turn that has already ended, or never ran", () => {
      chat.send(message("done"));
      vi.advanceTimersByTime(TURN_MS);
      const before = frames.length;
      chat.cancel("m1");
      chat.cancel("never-ran");
      expect(frames).toHaveLength(before);
      expect(busy).toEqual([true, false]);
    });
  });
});
