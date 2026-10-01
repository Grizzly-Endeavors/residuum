import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { ServerMessage } from "../src/lib/generated/protocol";
import type { ChatHistorySegment, RecentMessage } from "../src/lib/types";
import { chatHistorySegment, chatRoutes, createChatSimulator, type ChatSimulator } from "./chat";
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

  it("reports usage as zero totals", async () => {
    expect(await fetchJson(`${harness.baseUrl}/api/usage`)).toEqual({
      status: 200,
      body: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    });
  });
});

describe("chat turns", () => {
  const TURN_MS = 1500;
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

  function types(): string[] {
    return frames.map((f) => f.type);
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

  it("runs a turn: started, a note, a tool call, its result, the reply, ended", () => {
    chat.send(message("hello there"));
    expect(types()).toEqual(["turn_started"]);
    expect(frames[0]).toEqual({ type: "turn_started", reply_to: "m1" });

    vi.advanceTimersByTime(300);
    expect(types()).toEqual(["turn_started", "broadcast_response", "tool_call"]);

    vi.advanceTimersByTime(TURN_MS - 300);
    expect(types()).toEqual([
      "turn_started",
      "broadcast_response",
      "tool_call",
      "tool_result",
      "response",
      "turn_ended",
    ]);
    const [, , call, result, response] = frames;
    expect(call).toMatchObject({
      type: "tool_call",
      name: "memory_search",
      arguments: { query: "hello there", limit: 5 },
    });
    expect(result).toMatchObject({
      type: "tool_result",
      name: "memory_search",
      is_error: false,
    });
    expect(response).toEqual({ type: "response", reply_to: "m1", content: cannedResponses[0] });
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
      vi.advanceTimersByTime(TURN_MS);
    }
    const replies = frames.flatMap((f) => (f.type === "response" ? [f.content] : []));
    expect(replies).toEqual([...cannedResponses, cannedResponses[0]]);
  });

  it("records the whole turn in history when it ends", () => {
    chat.send(message("hello there"));
    vi.advanceTimersByTime(TURN_MS - 1);
    expect(recorded()).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(recorded().map((m) => m.role)).toEqual(["user", "assistant", "tool", "assistant"]);
    expect(recorded()[0]?.content).toBe("hello there");
    expect(recorded()[1]?.tool_calls?.[0]).toMatchObject({ name: "memory_search" });
    expect(recorded()[3]?.content).toBe(cannedResponses[0]);
    // Tagged with the turn's correlation id, so a page that saw it live can tell it's recorded.
    expect(recorded().map((m) => m.turn_id)).toEqual(["m1", "m1", "m1", "m1"]);
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
      expect(types()).toEqual(["turn_started", "broadcast_response", "tool_call"]);

      // Frames sent while the connection is down go nowhere.
      vi.advanceTimersByTime(RECONNECT_MS - DROP_MS);
      expect(types()).toHaveLength(3);

      vi.advanceTimersByTime(DROP_TURN_MS - RECONNECT_MS);
      expect(types().slice(3)).toEqual(["tool_result", "response", "turn_ended"]);
      expect(busy).toEqual([true, false]);
      expect(recorded()).toHaveLength(4);
    });

    it("drop finish: the turn ends while the page is away, so only history has it", () => {
      chat.send(message("drop finish now"));
      vi.advanceTimersByTime(DROP_FINISH_MS);
      expect(drops).toBe(1);
      expect(types()).toEqual(["turn_started", "broadcast_response", "tool_call"]);
      expect(busy).toEqual([true, false]);
      expect(recorded().map((m) => m.role)).toEqual(["user", "assistant", "tool", "assistant"]);

      // The page never comes back in the simulation's timeline: nothing more is sent.
      vi.advanceTimersByTime(DROP_TURN_MS);
      expect(types()).toHaveLength(3);
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

      vi.advanceTimersByTime(DROP_TURN_MS);
      const recent = chatHistorySegment(state, null) as ChatHistorySegment;
      expect(recent).toMatchObject({ kind: "recent", next_cursor: "ep-004" });
      // The turn that finished after the compression is the recent history.
      expect(recent.messages.map((m) => m.content)).toContain("drop compress it");
      const episode = chatHistorySegment(state, "ep-004");
      expect(episode?.messages.map((m) => m.content)).toContain("earlier");
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
