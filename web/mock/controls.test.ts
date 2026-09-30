import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ChatHistorySegment } from "../src/lib/types";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";

describe("test controls", () => {
  let harness: MockServerHarness;

  beforeEach(async () => {
    harness = await startMockServer();
  });

  afterEach(async () => {
    await harness.close();
  });

  const control = (path: string): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${harness.baseUrl}/api/mock/${path}`, { method: "POST" });

  async function recentMessages(agent: string): Promise<ChatHistorySegment["messages"]> {
    const res = await fetchJson(`${harness.baseUrl}/api/agents/${agent}/chat/history`);
    return (res.body as ChatHistorySegment).messages;
  }

  describe("missed-relay", () => {
    it("records a session's result and main's reply, then drops the agent's sockets", async () => {
      const socket = await harness.openSocket("/api/agents/scout/ws");
      const before = (await recentMessages("scout")).length;

      expect(await control("missed-relay")).toEqual({ status: 200, body: { ok: true } });
      await socket.closed;

      const messages = await recentMessages("scout");
      expect(messages).toHaveLength(before + 2);
      const [relayed, reply] = messages.slice(-2);
      expect(relayed).toMatchObject({
        role: "user",
        visibility: "background",
        agent_sender: { address: "spawned-research-3f9a", category: "spawned" },
      });
      expect(relayed?.content).toContain("Missed while you were away");
      expect(reply).toMatchObject({ role: "assistant", visibility: "background" });
    });

    it("acts on the agent named by ?agent=, and leaves the others alone", async () => {
      const scout = await harness.openSocket("/api/agents/scout/ws");
      const atlas = await harness.openSocket("/api/agents/atlas/ws");
      const scoutBefore = (await recentMessages("scout")).length;

      expect((await control("missed-relay?agent=atlas")).status).toBe(200);
      await atlas.closed;

      expect(await recentMessages("scout")).toHaveLength(scoutBefore);
      expect(scout.frames.map((frame) => frame.type)).not.toContain("error");
      const atlasMessages = await recentMessages("atlas");
      expect(atlasMessages.at(-2)?.content).toContain("Missed while you were away");
    });
  });

  describe("teammate-message", () => {
    it("lands a teammate's message in the agent's conversation, unread until the web UI opens its socket", async () => {
      const hub = await harness.openSocket("/api/hub/ws");
      await hub.nextOfType("agents_snapshot");

      expect(await control("teammate-message?agent=atlas")).toEqual({
        status: 200,
        body: { ok: true },
      });

      expect(await hub.nextOfType("agent_activity")).toEqual({
        type: "agent_activity",
        name: "atlas",
        busy: false,
        busy_since: null,
        unread: 1,
      });
      const [message, reply] = (await recentMessages("atlas")).slice(-2);
      expect(message).toMatchObject({ role: "user", visibility: "user" });
      expect(message?.content).toBe(
        "[Message from scout]\nCan you look over the wiki index when you get a chance?",
      );
      expect(reply).toMatchObject({
        role: "assistant",
        content: "scout asked me to check the wiki index. On it.",
      });
    });

    it("names the teammate with ?from=", async () => {
      await control("teammate-message?agent=atlas&from=drifter");
      const message = (await recentMessages("atlas")).at(-2);
      expect(message?.content).toContain("[Message from drifter]");
    });

    it("sends the reply to a connected client instead of marking the agent unread", async () => {
      const socket = await harness.openSocket("/api/agents/atlas/ws");
      const hub = await harness.openSocket("/api/hub/ws");
      await hub.nextOfType("agents_snapshot");

      await control("teammate-message?agent=atlas");

      expect(await socket.nextOfType("response")).toMatchObject({
        reply_to: "teammate",
        content: "scout asked me to check the wiki index. On it.",
      });
      await socket.settled();
      expect(harness.hub.agents.get("atlas")?.unread).toBe(0);
    });

    it("answers 404 without a known agent", async () => {
      const expected = { status: 404, body: { error: "mock: name an agent with ?agent=" } };
      expect(await control("teammate-message")).toEqual(expected);
      expect(await control("teammate-message?agent=ghost")).toEqual(expected);
    });
  });
});
