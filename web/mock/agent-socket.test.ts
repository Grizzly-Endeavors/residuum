import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parseClientMessage } from "./agent-socket";
import {
  startMockServer,
  type Frame,
  type MockServerHarness,
  type TestSocket,
} from "./test-support";

describe("parseClientMessage", () => {
  it.each([
    [{ type: "send_message", id: "m1", content: "hi" }],
    [{ type: "set_verbose", enabled: true }],
    [{ type: "ping" }],
    [{ type: "reload" }],
    [{ type: "server_command", name: "context", args: null }],
    [{ type: "inbox_add", body: "remember" }],
    [{ type: "cancel", reply_to: "m1" }],
    [{ type: "session_send_message", id: "s1", address: "a", content: "hi" }],
    [{ type: "session_stop", id: "s2", address: "a" }],
    [{ type: "watch_workspace", prefixes: ["team", ""] }],
  ])("reads %j as it is", (frame) => {
    expect(parseClientMessage(JSON.stringify(frame))).toEqual(frame);
  });

  it("reads a server command without arguments as having none", () => {
    expect(parseClientMessage('{"type":"server_command","name":"context"}')).toEqual({
      type: "server_command",
      name: "context",
      args: null,
    });
  });

  it.each([
    ["not json", "Unexpected token"],
    ["[]", "the request body must be a JSON object"],
    ["{}", "missing or invalid field `type`"],
    ['{"type":"teleport"}', "unknown variant `teleport`"],
    ['{"type":"send_message","id":"m1"}', "missing or invalid field `content`"],
    ['{"type":"send_message","id":5,"content":"x"}', "missing or invalid field `id`"],
    ['{"type":"set_verbose"}', "missing or invalid field `enabled`"],
    ['{"type":"cancel"}', "missing or invalid field `reply_to`"],
    ['{"type":"watch_workspace","prefixes":[1]}', "missing or invalid field `prefixes`"],
    ['{"type":"watch_workspace"}', "missing or invalid field `prefixes`"],
  ])("refuses %s", (raw, reason) => {
    expect(() => parseClientMessage(raw)).toThrow(reason);
  });
});

describe("agent socket", () => {
  let harness: MockServerHarness;
  let socket: TestSocket;

  beforeEach(async () => {
    harness = await startMockServer();
    socket = await harness.openSocket("/api/agents/atlas/ws");
  });

  afterEach(async () => {
    await harness.close();
  });

  describe("connecting", () => {
    it.each([
      ["drifter", "stopped"],
      ["brittle", "failed"],
    ])("is refused for %s with its state", async (agent, state) => {
      expect(await harness.refusedUpgrade(`/api/agents/${agent}/ws`)).toEqual({
        status: 409,
        body: { error: `${agent} is ${state}`, state },
      });
    });

    it("answers a ping with a pong", async () => {
      socket.send({ type: "ping" });
      expect(await socket.next()).toEqual({ type: "pong" });
    });

    it("opens a socket per agent, each with its own pages", async () => {
      const scout = await harness.openSocket("/api/agents/scout/ws");
      expect(harness.hub.agents.get("atlas")?.connectedClients()).toBe(1);
      expect(harness.hub.agents.get("scout")?.connectedClients()).toBe(1);
      await scout.close();
    });

    it("drops a page's connections when the agent's state asks to", async () => {
      harness.hub.agents.get("atlas")?.state.dropSockets();
      await socket.closed;
    });
  });

  describe("frames it can't read", () => {
    it.each([
      ["nonsense", "malformed message: Unexpected token"],
      [JSON.stringify({ type: "teleport" }), "malformed message: unknown variant `teleport`"],
      [
        JSON.stringify({ type: "send_message", id: "m1" }),
        "malformed message: missing or invalid field `content`",
      ],
    ])("answers %s with an error frame", async (raw, message) => {
      socket.sendRaw(raw);
      const frame = await socket.nextOfType("error");
      expect(frame).toMatchObject({ reply_to: null, details: null });
      expect(String(frame.message)).toContain(message);
    });

    it("keeps serving after one", async () => {
      socket.sendRaw("nonsense");
      socket.send({ type: "ping" });
      await socket.nextOfType("pong");
    });
  });

  describe("commands", () => {
    it("acknowledges a server command and an inbox add with a notice", async () => {
      socket.send({ type: "server_command", name: "context", args: null });
      expect(await socket.nextOfType("notice")).toEqual({
        type: "notice",
        message: "Command 'context' executed. (mock)",
      });
      socket.send({ type: "inbox_add", body: "remember the milk" });
      expect(await socket.nextOfType("notice")).toEqual({
        type: "notice",
        message: "[inbox] item added",
      });
    });

    it("reloads: says so, then reports it finished", async () => {
      socket.send({ type: "reload" });
      expect(await socket.next()).toEqual({ type: "reloading" });
      expect(await socket.nextOfType("notice", 3000)).toEqual({
        type: "notice",
        message: "Configuration reloaded successfully.",
      });
    });

    it("accepts verbose being set either way without a word", async () => {
      socket.send({ type: "set_verbose", enabled: true });
      socket.send({ type: "set_verbose", enabled: false });
      expect(await socket.settled()).toEqual([{ type: "pong" }]);
    });

    it("accepts workspace paths quietly, and refuses one outside the workspace", async () => {
      socket.send({ type: "watch_workspace", prefixes: ["team/wiki", ""] });
      expect(await socket.settled()).toEqual([{ type: "pong" }]);
      socket.send({ type: "watch_workspace", prefixes: ["ok", "../secrets"] });
      const frames = await socket.settled();
      expect(frames[0]).toEqual({
        type: "error",
        reply_to: null,
        message:
          'Couldn\'t watch the workspace: can\'t watch "../secrets": watch paths must stay inside the workspace (no "..").',
        details: null,
      });
    });

    it("starts a session when the message begins with spawn", async () => {
      socket.send({ type: "send_message", id: "m1", content: "spawn dig into the wiki" });
      const started = await socket.nextOfType("session_started");
      expect(started.session).toMatchObject({
        category: "spawned",
        purpose: "dig into the wiki",
        spawner: "main",
      });
      socket.send({ type: "send_message", id: "m2", content: "Spawn" });
      const bare = await socket.next(
        (f) =>
          f.type === "session_started" && (f.session as Frame).purpose === "Look into something",
      );
      expect(bare).toBeDefined();
    });

    it("stops a turn when asked to cancel it", async () => {
      socket.send({ type: "send_message", id: "m1", content: "a long one" });
      await socket.nextOfType("turn_started");
      socket.send({ type: "cancel", reply_to: "m1" });
      expect(await socket.nextOfType("turn_ended")).toEqual({ type: "turn_ended", reply_to: "m1" });
      const rest = await socket.quietFrames(1800);
      expect(rest.map((f) => f.type)).not.toContain("response");
      expect(harness.hub.agents.get("atlas")?.busySince).toBeNull();
    });
  });

  describe("session commands", () => {
    it("fails a message to a session that is busy, to the sender only", async () => {
      const other = await harness.openSocket("/api/agents/atlas/ws");
      socket.send({
        type: "session_send_message",
        id: "s1",
        address: "spawned-research-3f9a",
        content: "are you busy",
      });
      const failed = await socket.nextOfType("session_command_failed");
      expect(failed).toMatchObject({ id: "s1", address: "spawned-research-3f9a", code: "busy" });
      expect(await other.quietFrames()).toEqual([]);
    });

    it("delivers a message to a live session", async () => {
      socket.send({
        type: "session_send_message",
        id: "s2",
        address: "spawned-research-3f9a",
        content: "keep going",
      });
      expect(await socket.nextOfType("session_message_delivered")).toEqual({
        type: "session_message_delivered",
        id: "s2",
        address: "spawned-research-3f9a",
        outcome: "live",
      });
    });

    it("fails a message to a session nobody knows", async () => {
      socket.send({ type: "session_send_message", id: "s3", address: "nobody", content: "hi" });
      expect(await socket.nextOfType("session_command_failed")).toMatchObject({
        id: "s3",
        address: "nobody",
        code: "unknown_address",
      });
    });

    it("stops a live session, and fails to stop one nobody knows", async () => {
      socket.send({ type: "session_stop", id: "s4", address: "spawned-research-3f9a" });
      expect(await socket.nextOfType("session_stop_requested")).toEqual({
        type: "session_stop_requested",
        id: "s4",
        address: "spawned-research-3f9a",
      });
      socket.send({ type: "session_stop", id: "s5", address: "nobody" });
      expect(await socket.nextOfType("session_command_failed")).toMatchObject({
        id: "s5",
        code: "not_live",
      });
    });
  });

  describe("a chat turn over the socket", () => {
    it("shows tool calls only to pages that turned verbose mode on", async () => {
      const verbose = await harness.openSocket("/api/agents/atlas/ws");
      verbose.send({ type: "set_verbose", enabled: true });
      await verbose.settled();

      socket.send({ type: "send_message", id: "m1", content: "look it up" });
      await verbose.nextOfType("turn_ended");
      await socket.nextOfType("turn_ended");

      const types = (frames: Frame[]): string[] => frames.map((f) => String(f.type));
      expect(types(socket.frames)).toEqual([
        "turn_started",
        "broadcast_response",
        "turn_usage",
        "turn_usage",
        "response",
        "turn_ended",
      ]);
      expect(types(verbose.frames)).toEqual([
        "pong",
        "turn_started",
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
      const call = verbose.frames.find((f) => f.type === "tool_call");
      expect(call).toMatchObject({ name: "memory_search", arguments: { query: "look it up" } });
    });

    it("tells the hub the agent was busy, then records the turn in its history", async () => {
      const hubSocket = await harness.openSocket("/api/hub/ws");
      await hubSocket.nextOfType("agents_snapshot");
      socket.send({ type: "send_message", id: "m1", content: "hello" });
      expect(await hubSocket.nextOfType("agent_activity")).toEqual({
        type: "agent_activity",
        name: "atlas",
        busy: true,
        busy_since: expect.any(String) as unknown,
        unread: 0,
      });
      await socket.nextOfType("turn_ended");
      expect(await hubSocket.nextOfType("agent_activity")).toMatchObject({
        busy: false,
        busy_since: null,
      });
      const history = harness.hub.agents.get("atlas")?.state.extraRecent ?? [];
      // Every agent but scout opens its conversation with a greeting.
      expect(history.slice(-7).map((m) => m.role)).toEqual([
        "user",
        "assistant",
        "tool",
        "assistant",
        "tool",
        "tool",
        "assistant",
      ]);
      expect(history.at(-7)?.content).toBe("hello");
    });
  });
});
