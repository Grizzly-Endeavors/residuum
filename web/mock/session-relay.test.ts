import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { fetchJson, startMockServer, type Frame, type MockServerHarness } from "./test-support";
import type { TestSocket } from "./test-support";

/** What the hub says to a frame it can't read; it answers one, so a test can tell the hub has handled what came before. */
const UNREADABLE = { type: "notice", level: "warn" };

describe("the hub socket's session relay", () => {
  let mock: MockServerHarness;
  let hubSocket: TestSocket;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
    hubSocket = await mock.openSocket("/api/hub/ws");
    await hubSocket.nextOfType("agents_snapshot");
  });

  afterEach(async () => {
    await mock.close();
  });

  /** Start a session for `artifact` on `agent` over HTTP, as the SDK does, and answer its address. */
  async function startSession(agent: string, artifact: string): Promise<string> {
    const res = await fetchJson(`${mock.baseUrl}/api/agents/${agent}/sessions`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Residuum-Artifact": artifact },
      body: JSON.stringify({ prompt: "Draft the page." }),
    });
    expect(res.status).toBe(202);
    return (res.body as { address: string }).address;
  }

  /** Wait until the hub has handled everything the socket sent so far: it answers a message it can't read. */
  async function handled(socket: TestSocket = hubSocket): Promise<void> {
    socket.send({ type: "nonsense" });
    await socket.next((frame) => frame.type === "notice");
  }

  const sessionFrames = (socket: TestSocket): Frame[] =>
    socket.frames.filter((frame) => frame.type === "session_frame");

  /** The session frames `socket` is sent in the next `ms` milliseconds, to show that no more come. */
  const relayedWithin = async (socket: TestSocket, ms: number): Promise<Frame[]> =>
    (await socket.quietFrames(ms)).filter((frame) => frame.type === "session_frame");

  /** The type of the frame a `session_frame` carries. */
  const carried = (frame: Frame): string => (frame.frame as { type: string }).type;

  /** Wait for `socket` to be sent a session frame of `type`. */
  const nextCarrying = (socket: TestSocket, type: string): Promise<Frame> =>
    socket.next((frame) => frame.type === "session_frame" && carried(frame) === type);

  describe("a subscription", () => {
    it("is acknowledged with its kind and what it names", async () => {
      hubSocket.send({ type: "subscribe_session", agent: "atlas", address: "spawned-x" });
      expect(await hubSocket.nextOfType("subscribed")).toEqual({
        type: "subscribed",
        kind: "session",
        agent: "atlas",
        address: "spawned-x",
      });
      hubSocket.send({ type: "subscribe_artifact_sessions", artifact: "tip-splitter" });
      expect(await hubSocket.nextOfType("subscribed")).toEqual({
        type: "subscribed",
        kind: "artifact_sessions",
        artifact: "tip-splitter",
      });
    });

    it("is refused with a notice and no acknowledgement for an unknown agent, and accepted for a stopped one", async () => {
      hubSocket.send({ type: "subscribe_session", agent: "ghost", address: "spawned-x" });
      expect(await hubSocket.nextOfType("notice")).toEqual({
        type: "notice",
        level: "warn",
        message: 'Couldn\'t follow sessions on "ghost": no agent has that name.',
      });
      expect(hubSocket.frames.some((frame) => frame.type === "subscribed")).toBe(false);

      hubSocket.send({ type: "subscribe_session", agent: "drifter", address: "spawned-x" });
      expect(await hubSocket.nextOfType("subscribed")).toMatchObject({ agent: "drifter" });
    });

    it.each([
      { type: "subscribe_session", agent: "atlas" },
      { type: "subscribe_artifact_sessions" },
      { type: "unsubscribe_session", address: "x" },
      { type: "unsubscribe_artifact_sessions", artifact: 7 },
    ])("answers a message it can't read, %j, with a warning", async (message) => {
      hubSocket.send(message);
      expect(await hubSocket.nextOfType("notice")).toMatchObject(UNREADABLE);
      expect(hubSocket.frames.some((frame) => frame.type === "subscribed")).toBe(false);
    });
  });

  describe("an artifact's sessions", () => {
    it("arrive whole, tool frames included, exactly as the agent's own socket sends them", async () => {
      const agentSocket = await mock.openSocket("/api/agents/atlas/ws");
      agentSocket.send({ type: "set_verbose", enabled: true });
      await agentSocket.settled();
      hubSocket.send({ type: "subscribe_artifact_sessions", artifact: "tip-splitter" });
      await hubSocket.nextOfType("subscribed");

      const address = await startSession("atlas", "tip-splitter");
      await nextCarrying(hubSocket, "session_turn_ended");

      const relayed = sessionFrames(hubSocket);
      expect(relayed.map(carried)).toEqual([
        "session_started",
        "session_state_changed",
        "session_turn_started",
        "session_broadcast_response",
        "session_tool_call",
        "session_tool_result",
        "session_response",
        "session_turn_ended",
        "session_state_changed",
      ]);
      expect(relayed.every((frame) => frame.agent === "atlas")).toBe(true);
      expect(relayed[0]?.frame).toMatchObject({
        session: { address, source_label: "artifact:tip-splitter" },
      });

      // The hub socket has no verbose flag, and what it carries is what the agent's socket sent.
      await agentSocket.settled();
      const onAgentSocket = agentSocket.frames.filter((frame) =>
        (frame.type as string).startsWith("session_"),
      );
      expect(relayed.map((frame) => frame.frame)).toEqual(onAgentSocket);
    });

    it("include sessions that start later, on any agent, and leave other sessions out", async () => {
      hubSocket.send({ type: "subscribe_artifact_sessions", artifact: "tip-splitter" });
      await hubSocket.nextOfType("subscribed");

      await startSession("atlas", "tip-splitter");
      await startSession("scout", "tip-splitter");
      await startSession("scout", "another-artifact");
      await nextCarrying(hubSocket, "session_turn_ended");
      await nextCarrying(hubSocket, "session_turn_ended");

      const started = sessionFrames(hubSocket).filter(
        (frame) => carried(frame) === "session_started",
      );
      expect(started.map((frame) => frame.agent).sort()).toEqual(["atlas", "scout"]);
      for (const frame of sessionFrames(hubSocket)) {
        const inner = frame.frame as { address?: string; session?: { address: string } };
        const address = inner.address ?? inner.session?.address;
        expect(address).toContain("tip-splitter");
      }
    });

    it("stop arriving once the page unsubscribes", async () => {
      hubSocket.send({ type: "subscribe_artifact_sessions", artifact: "tip-splitter" });
      await hubSocket.nextOfType("subscribed");
      hubSocket.send({ type: "unsubscribe_artifact_sessions", artifact: "tip-splitter" });
      await handled();

      await startSession("atlas", "tip-splitter");
      expect(sessionFrames(hubSocket)).toEqual([]);
      expect(await relayedWithin(hubSocket, 200)).toEqual([]);
    });

    it("reach only the connection that subscribed, and end with it", async () => {
      hubSocket.send({ type: "subscribe_artifact_sessions", artifact: "tip-splitter" });
      await hubSocket.nextOfType("subscribed");
      await hubSocket.close();

      const later = await mock.openSocket("/api/hub/ws");
      await later.nextOfType("agents_snapshot");
      await startSession("atlas", "tip-splitter");
      expect(await relayedWithin(later, 200)).toEqual([]);
    });
  });

  describe("one session", () => {
    it("is followed from the subscription on, and only that session", async () => {
      const followed = await startSession("atlas", "tip-splitter");
      await startSession("scout", "tip-splitter");
      hubSocket.send({ type: "subscribe_session", agent: "atlas", address: followed });
      await hubSocket.nextOfType("subscribed");
      await handled();

      const res = await fetchJson(
        `${mock.baseUrl}/api/agents/atlas/sessions/${followed}/messages`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-Residuum-Artifact": "tip-splitter" },
          body: JSON.stringify({ content: "Add a tip field." }),
        },
      );
      expect(res.status).toBe(200);
      await nextCarrying(hubSocket, "session_turn_ended");

      const relayed = sessionFrames(hubSocket);
      expect(relayed.length).toBeGreaterThan(0);
      expect(relayed.every((frame) => frame.agent === "atlas")).toBe(true);
      expect(
        relayed.every((frame) => (frame.frame as { address: string }).address === followed),
      ).toBe(true);
    });

    it("stops arriving once the page unsubscribes", async () => {
      const address = await startSession("atlas", "tip-splitter");
      hubSocket.send({ type: "subscribe_session", agent: "atlas", address });
      await hubSocket.nextOfType("subscribed");
      hubSocket.send({ type: "unsubscribe_session", agent: "atlas", address });
      await handled();

      await fetchJson(`${mock.baseUrl}/api/agents/atlas/sessions/${address}/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Residuum-Artifact": "tip-splitter" },
        body: JSON.stringify({ content: "Add a tip field." }),
      });
      expect(await relayedWithin(hubSocket, 200)).toEqual([]);
    });
  });

  describe("the lag control", () => {
    it("tells the pages that follow sessions, and only them, that frames were lost", async () => {
      const idle = await mock.openSocket("/api/hub/ws");
      await idle.nextOfType("agents_snapshot");
      hubSocket.send({ type: "subscribe_artifact_sessions", artifact: "tip-splitter" });
      await hubSocket.nextOfType("subscribed");

      const res = await fetchJson(`${mock.baseUrl}/api/mock/session-relay-lag`, { method: "POST" });
      expect(res).toEqual({ status: 200, body: { notified: 1 } });
      expect(await hubSocket.nextOfType("session_relay_lagged")).toEqual({
        type: "session_relay_lagged",
      });
      expect(await relayedWithin(idle, 200)).toEqual([]);
    });
  });
});
