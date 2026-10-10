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

  describe("fix-agent", () => {
    const start = async (agent: string): Promise<unknown> =>
      (
        await fetchJson(`${harness.baseUrl}/api/hub/agents/${agent}/start`, {
          method: "POST",
        })
      ).body;

    it("lets an agent that failed every start start, once its settings are fixed", async () => {
      expect(await start("brittle")).toMatchObject({ state: "failed" });

      expect(await control("fix-agent?agent=brittle")).toEqual({ status: 200, body: { ok: true } });

      expect(await start("brittle")).toMatchObject({ state: "running", last_error: null });
    });

    it("answers 404 without an agent it knows", async () => {
      expect((await control("fix-agent?agent=ghost")).status).toBe(404);
      expect((await control("fix-agent")).status).toBe(404);
    });
  });

  describe("hub-socket", () => {
    const setOnline = (online: unknown): Promise<{ status: number; body: unknown }> =>
      fetchJson(`${harness.baseUrl}/api/mock/hub-socket`, {
        method: "POST",
        body: JSON.stringify({ online }),
      });

    it("drops the hub's pages and refuses new ones until it is back online", async () => {
      const page = await harness.openSocket("/api/hub/ws");

      expect(await setOnline(false)).toEqual({ status: 200, body: { online: false } });
      await page.closed;
      expect((await harness.refusedUpgrade("/api/hub/ws")).status).toBe(409);

      expect(await setOnline(true)).toEqual({ status: 200, body: { online: true } });
      const again = await harness.openSocket("/api/hub/ws");
      expect((await again.nextOfType("hub_boot")).type).toBe("hub_boot");
    });

    it("comes back online on reset", async () => {
      await setOnline(false);
      expect((await control("reset")).status).toBe(200);
      const page = await harness.openSocket("/api/hub/ws");
      expect((await page.nextOfType("agents_snapshot")).type).toBe("agents_snapshot");
    });

    it("refuses anything but true or false", async () => {
      expect((await setOnline("no")).status).toBe(422);
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
      expect(message).toMatchObject({
        role: "user",
        visibility: "background",
        agent_sender: { address: "agent:scout", category: "teammate" },
      });
      expect(message?.content).toBe(
        "[Message from teammate agent:scout, not the user. Your response in this turn is not " +
          'shown to them; to reply, call message_agent with to="agent:scout".]\n' +
          "Can you look over the wiki index when you get a chance?",
      );
      expect(reply).toMatchObject({
        role: "assistant",
        content: "scout asked me to check the wiki index. On it.",
      });
    });

    it("names the teammate with ?from=", async () => {
      await control("teammate-message?agent=atlas&from=drifter");
      const message = (await recentMessages("atlas")).at(-2);
      expect(message?.content).toContain("[Message from teammate agent:drifter,");
      expect(message?.agent_sender?.address).toBe("agent:drifter");
    });

    it("sends a connected client the turn the reply came in, as the backend does, and doesn't mark the agent unread", async () => {
      const socket = await harness.openSocket("/api/agents/atlas/ws");
      const hub = await harness.openSocket("/api/hub/ws");
      await hub.nextOfType("agents_snapshot");

      await control("teammate-message?agent=atlas");

      // A turn no person started: it opens, the reply streams in and arrives whole, it ends.
      const started = await socket.nextOfType("turn_started");
      const turnId = started.reply_to;
      expect(turnId).not.toBe("teammate");
      expect(started.origin).toEqual({ endpoint: "background", visibility: "background" });
      const text = await socket.nextOfType("text_delta");
      expect(text).toMatchObject({ reply_to: turnId, call: 0 });
      expect(await socket.nextOfType("response")).toEqual({
        type: "response",
        reply_to: turnId,
        call: 0,
        endpoint: "",
        content: "scout asked me to check the wiki index. On it.",
      });
      expect(await socket.nextOfType("turn_ended")).toEqual({
        type: "turn_ended",
        reply_to: turnId,
      });
      await socket.settled();
      // The message that started the turn has no frame: the backend announces only a person's.
      expect(socket.frames.some((frame) => frame.type === "user_message")).toBe(false);
      expect(harness.hub.agents.get("atlas")?.unread).toBe(0);

      // History holds both messages under the turn's id.
      const [message, reply] = (await recentMessages("atlas")).slice(-2);
      expect(message).toMatchObject({ role: "user", turn_id: turnId });
      expect(reply).toMatchObject({ role: "assistant", turn_id: turnId });
    });

    it("answers 404 without a known agent", async () => {
      const expected = { status: 404, body: { error: "mock: name an agent with ?agent=" } };
      expect(await control("teammate-message")).toEqual(expected);
      expect(await control("teammate-message?agent=ghost")).toEqual(expected);
    });
  });

  describe("telegram-message", () => {
    const post = (path: string, body?: unknown): Promise<{ status: number; body: unknown }> =>
      fetchJson(`${harness.baseUrl}/api/mock/${path}`, {
        method: "POST",
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });

    beforeEach(async () => {
      // No waiting: the turn runs to its end at once.
      await post("delays", { scale: 0 });
    });

    it("tells a connected page of the person's message, then runs a turn that answers on Telegram", async () => {
      const socket = await harness.openSocket("/api/agents/atlas/ws");
      expect(
        await post("telegram-message?agent=atlas", { content: "Is the build green?", name: "Sam" }),
      ).toEqual({ status: 200, body: { ok: true } });

      const sender = {
        name: "Sam",
        id: "42",
        interface: "telegram",
        location: "direct message",
      };
      expect(await socket.nextOfType("user_message")).toMatchObject({
        content: "Is the build green?",
        sender,
        endpoint: "telegram",
      });
      expect(await socket.nextOfType("turn_started")).toMatchObject({
        origin: { endpoint: "telegram", sender, visibility: "user" },
      });
      expect(await socket.nextOfType("response")).toMatchObject({ endpoint: "telegram" });
      await socket.nextOfType("turn_ended");

      const messages = await recentMessages("atlas");
      expect(messages.find((m) => m.content === "Is the build green?")).toMatchObject({
        role: "user",
        sender,
      });
    });

    it("asks about the routing doc, from Alex, when told nothing", async () => {
      const socket = await harness.openSocket("/api/agents/atlas/ws");
      await post("telegram-message?agent=atlas");
      expect(await socket.nextOfType("user_message")).toMatchObject({
        content: expect.stringContaining("routing doc") as string,
        sender: { name: "Alex" },
      });
    });

    it("answers 404 without a known agent, and 422 for text that isn't", async () => {
      const missing = { status: 404, body: { error: "mock: name an agent with ?agent=" } };
      expect(await post("telegram-message")).toEqual(missing);
      expect(await post("telegram-message?agent=ghost")).toEqual(missing);
      expect((await post("telegram-message?agent=atlas", { content: 3 })).status).toBe(422);
    });
  });

  describe("reset", () => {
    const agentNames = async (): Promise<string[]> => {
      const res = await fetchJson(`${harness.baseUrl}/api/hub/agents`);
      return (res.body as { agents: { name: string }[] }).agents.map((agent) => agent.name);
    };

    const resetWith = (body: unknown): Promise<{ status: number; body: unknown }> =>
      fetchJson(`${harness.baseUrl}/api/mock/reset`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });

    it("starts over with no agents when asked for setup, and with the scenario's otherwise", async () => {
      expect(await resetWith({ setup: true })).toEqual({ status: 200, body: { ok: true } });
      expect(await agentNames()).toEqual([]);

      expect(await control("reset")).toEqual({ status: 200, body: { ok: true } });
      expect(await agentNames()).toEqual(["atlas", "brittle", "drifter", "scout"]);
    });

    it("answers 422 when setup isn't a boolean", async () => {
      expect(await resetWith({ setup: "yes" })).toEqual({
        status: 422,
        body: { error: "mock: `setup` must be true or false" },
      });
    });
  });

  describe("rebuild", () => {
    it("counts rebuilds of the app, and a reset starts over from the build as it is", async () => {
      expect(await control("rebuild")).toEqual({ status: 200, body: { rebuilds: 1 } });
      expect(await control("rebuild")).toEqual({ status: 200, body: { rebuilds: 2 } });
      expect((await control("reset")).status).toBe(200);
      expect(await control("rebuild")).toEqual({ status: 200, body: { rebuilds: 1 } });
    });
  });
});
