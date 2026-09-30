import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { MOCK_BRITTLE_ERROR } from "./constants";
import {
  fetchJson,
  startMockServer,
  type Frame,
  type MockServerHarness,
  type TestSocket,
} from "./test-support";

type Body = Record<string, unknown>;

describe("hub", () => {
  let harness: MockServerHarness;
  let hubSocket: TestSocket;

  async function open(options: { seed?: boolean } = {}): Promise<void> {
    harness = await startMockServer(options);
    hubSocket = await harness.openSocket("/api/hub/ws");
  }

  beforeEach(async () => {
    await open();
    await hubSocket.nextOfType("agents_snapshot");
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/hub${path}`;

  async function request(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<{ status: number; body: Body }> {
    const res = await fetchJson(url(path), {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: res.status, body: res.body as Body };
  }

  const names = (agents: unknown): string[] => (agents as { name: string }[]).map((a) => a.name);

  describe("agent list and status", () => {
    it("lists the agents by name, with the state each is in", async () => {
      const { status, body } = await request("GET", "/agents");
      expect(status).toBe(200);
      expect(names(body.agents)).toEqual(["atlas", "brittle", "drifter", "scout"]);
      const byName = Object.fromEntries(
        (body.agents as { name: string; state: string }[]).map((a) => [a.name, a.state]),
      );
      expect(byName).toEqual({
        atlas: "running",
        brittle: "failed",
        drifter: "stopped",
        scout: "running",
      });
    });

    it("carries the failure only for a failed agent", async () => {
      const agents = (await request("GET", "/agents")).body.agents as {
        name: string;
        last_error: { message: string } | null;
      }[];
      expect(agents.find((a) => a.name === "brittle")?.last_error?.message).toBe(
        MOCK_BRITTLE_ERROR,
      );
      expect(agents.filter((a) => a.last_error !== null).map((a) => a.name)).toEqual(["brittle"]);
    });

    it("reports the version, uptime, tunnel and how many agents are in each state", async () => {
      const { body } = await request("GET", "/status");
      expect(body).toMatchObject({
        version: "0.0.0-mock",
        tunnel: { status: "disconnected" },
        agents: { starting: 0, running: 2, stopped: 1, failed: 1 },
      });
      expect(body.uptime_secs).toEqual(expect.any(Number));
    });
  });

  describe("create", () => {
    it("creates an agent, tells the hub socket, and lists it", async () => {
      const { status, body } = await request("POST", "/agents", {
        name: "newbie",
        description: "Fresh",
        models_from: "atlas",
        a2a_visibility: "public",
      });
      expect(status).toBe(201);
      expect(body).toMatchObject({
        name: "newbie",
        state: "running",
        role: "Fresh",
        a2a_visibility: "public",
      });
      const frame = await hubSocket.nextOfType("agent_created");
      expect(frame).toMatchObject({ agent: { name: "newbie" }, by: "user" });
      expect(names((await request("GET", "/agents")).body.agents)).toContain("newbie");
    });

    it("accepts providers in place of another agent to copy them from", async () => {
      const { status } = await request("POST", "/agents", { name: "newbie", providers_toml: "" });
      expect(status).toBe(201);
    });

    it.each([
      [{ name: "Bad Name", models_from: "atlas" }, "agent name 'Bad Name' must contain only"],
      [{ name: "atlas", models_from: "scout" }, "an agent named 'atlas' already exists"],
      [{ name: "newbie", models_from: "ghost" }, "no agent named 'ghost'"],
      [{ name: "newbie" }, "give models_from or providers_toml"],
      [{ models_from: "atlas" }, "missing field `name`"],
    ])("refuses %j", async (payload, message) => {
      const { status, body } = await request("POST", "/agents", payload);
      expect(status).toBeGreaterThanOrEqual(400);
      expect(String(body.error)).toContain(message);
    });

    it("answers 400 for a body that isn't JSON", async () => {
      const res = await fetchJson(url("/agents"), { method: "POST", body: "nope" });
      expect(res.status).toBe(400);
      expect(String((res.body as Body).error)).toContain(
        "the request body isn't valid for this route",
      );
    });
  });

  describe("change", () => {
    it("patches autostart and visibility, and tells the hub socket", async () => {
      const { status, body } = await request("PATCH", "/agents/scout", {
        autostart: false,
        a2a_visibility: "public",
      });
      expect(status).toBe(200);
      expect(body).toMatchObject({ name: "scout", autostart: false, a2a_visibility: "public" });
      expect(await hubSocket.nextOfType("agent_state")).toMatchObject({
        agent: { name: "scout", autostart: false },
      });
    });

    it("needs something to change, and an agent that exists", async () => {
      const empty = await request("PATCH", "/agents/scout", {});
      expect(empty.status).toBe(400);
      expect(empty.body.error).toBe(
        "the request must set at least one of autostart or a2a_visibility",
      );
      const ghost = await request("PATCH", "/agents/ghost", { autostart: true });
      expect(ghost).toEqual({ status: 404, body: { error: "no agent named 'ghost'" } });
    });
  });

  describe("start, stop and restart", () => {
    /** The states the hub socket was told about, once it has seen one that is `until`. */
    async function statesUntil(until: string): Promise<string[]> {
      await hubSocket.next((f) => f.type === "agent_state" && (f.agent as Body).state === until);
      return hubSocket.frames.flatMap((f) =>
        f.type === "agent_state" ? [(f.agent as Body).state as string] : [],
      );
    }

    it("starts a stopped agent through starting to running", async () => {
      const { status, body } = await request("POST", "/agents/drifter/start");
      expect(status).toBe(200);
      expect(body).toMatchObject({ name: "drifter", state: "running" });
      expect(await statesUntil("running")).toEqual(["starting", "running"]);
    });

    it("leaves a running agent running when asked to start it", async () => {
      const { body } = await request("POST", "/agents/scout/start");
      expect(body).toMatchObject({ state: "running" });
      expect(await hubSocket.quietFrames()).toEqual([]);
    });

    it("stops an agent, drops its sockets, and refuses its live routes", async () => {
      const agentSocket = await harness.openSocket("/api/agents/scout/ws");
      const { body } = await request("POST", "/agents/scout/stop");
      expect(body).toMatchObject({ name: "scout", state: "stopped" });
      await agentSocket.closed;
      expect(await hubSocket.nextOfType("agent_state")).toMatchObject({
        agent: { name: "scout", state: "stopped" },
      });
      const live = await fetchJson(`${harness.baseUrl}/api/agents/scout/status`);
      expect(live).toEqual({
        status: 409,
        body: { error: "scout is stopped", state: "stopped" },
      });
    });

    it("restarts a running agent through starting", async () => {
      const { body } = await request("POST", "/agents/atlas/restart");
      expect(body).toMatchObject({ name: "atlas", state: "running" });
      expect(await statesUntil("running")).toEqual(["starting", "running"]);
    });

    it("fails again whenever brittle is started", async () => {
      const { status, body } = await request("POST", "/agents/brittle/start");
      expect(status).toBe(200);
      expect(body).toMatchObject({
        name: "brittle",
        state: "failed",
        last_error: { message: MOCK_BRITTLE_ERROR },
      });
      expect(await statesUntil("failed")).toEqual(["starting", "failed"]);
    });

    it("answers 404 for an agent that doesn't exist", async () => {
      expect(await request("POST", "/agents/ghost/start")).toEqual({
        status: 404,
        body: { error: "no agent named 'ghost'" },
      });
    });

    it("stops every running agent at once, and leaves the others as they are", async () => {
      const { status, body } = await request("POST", "/stop-all");
      expect(status).toBe(200);
      expect(names(body.stopped)).toEqual(["atlas", "scout"]);
      expect(body.failed).toEqual([]);
      const { body: hubStatus } = await request("GET", "/status");
      expect(hubStatus.agents).toEqual({ starting: 0, running: 0, stopped: 3, failed: 1 });
    });
  });

  describe("delete and restore", () => {
    it("deletes an agent with a checkpoint, lists it as deleted, and restores it", async () => {
      const deleted = await request("DELETE", "/agents/atlas");
      expect(deleted.status).toBe(200);
      expect(deleted.body).toMatchObject({ deleted: true });
      const checkpointId = String(deleted.body.checkpoint_id);
      expect(await hubSocket.nextOfType("agent_deleted")).toEqual({
        type: "agent_deleted",
        name: "atlas",
        by: "user",
      });
      expect(names((await request("GET", "/agents")).body.agents)).not.toContain("atlas");

      const list = (await request("GET", "/agents/deleted")).body.agents as Body[];
      expect(list).toEqual([
        { name: "atlas", deleted_at: expect.any(String) as unknown, checkpoint_id: checkpointId },
      ]);

      const restored = await request("POST", "/agents/restore", {
        name: "atlas",
        checkpoint_id: checkpointId,
      });
      expect(restored.status).toBe(201);
      expect(restored.body).toMatchObject({ name: "atlas", state: "running" });
      expect(await hubSocket.nextOfType("agent_restored")).toMatchObject({
        agent: { name: "atlas" },
        by: "user",
      });
      expect((await request("GET", "/agents/deleted")).body.agents).toEqual([]);
    });

    it("keeps a deleted agent's conversation for the restore", async () => {
      const agent = harness.hub.agents.get("atlas");
      agent?.state.extraRecent.push({
        role: "user",
        content: "remember me",
        timestamp: new Date().toISOString(),
        visibility: "user",
      });
      await request("DELETE", "/agents/atlas");
      await request("POST", "/agents/restore", { name: "atlas" });
      const history = await fetchJson(`${harness.baseUrl}/api/agents/atlas/chat/history`);
      const messages = (history.body as { messages: { content: string }[] }).messages;
      expect(messages.at(-1)?.content).toBe("remember me");
    });

    it("refuses a restore it can't do", async () => {
      await request("DELETE", "/agents/atlas");
      expect(
        (await request("POST", "/agents/restore", { name: "atlas", checkpoint_id: "wrong" })).body,
      ).toEqual({ error: "atlas has no checkpoint 'wrong' to restore from" });
      expect((await request("POST", "/agents/restore", { name: "ghost" })).body).toEqual({
        error: "there is no deleted agent named 'ghost' to restore",
      });
      expect((await request("POST", "/agents/restore", { name: "scout" })).status).toBe(409);
      expect((await request("POST", "/agents/restore", { name: "BAD" })).status).toBe(400);
      expect((await request("POST", "/agents/restore", {})).status).toBe(400);
    });

    it("answers 404 when deleting an agent that doesn't exist", async () => {
      expect(await request("DELETE", "/agents/ghost")).toEqual({
        status: 404,
        body: { error: "no agent named 'ghost'" },
      });
    });
  });

  describe("unknown hub endpoints", () => {
    it("answers 404 naming the endpoint", async () => {
      expect(await request("GET", "/agents/scout")).toEqual({
        status: 404,
        body: { error: "mock: unknown endpoint GET /api/hub/agents/scout" },
      });
    });
  });
});

describe("hub socket", () => {
  let harness: MockServerHarness;

  afterEach(async () => {
    await harness.close();
  });

  it("sends the agents by name first, as a snapshot", async () => {
    harness = await startMockServer();
    const socket = await harness.openSocket("/api/hub/ws");
    const snapshot = await socket.next();
    expect(snapshot.type).toBe("agents_snapshot");
    expect((snapshot.agents as { name: string; state: string }[]).map((a) => a.name)).toEqual([
      "atlas",
      "brittle",
      "drifter",
      "scout",
    ]);
    expect(snapshot.agents).toContainEqual({
      name: "brittle",
      state: "failed",
      last_error: { message: MOCK_BRITTLE_ERROR, at: expect.any(String) as unknown },
      autostart: true,
      role: "Has a broken model config",
      a2a_visibility: "private",
    });
  });

  it("follows the snapshot with the activity of each agent that is busy or has unread messages", async () => {
    harness = await startMockServer();
    const atlas = harness.hub.agents.get("atlas");
    const scout = harness.hub.agents.get("scout");
    if (!atlas || !scout) throw new Error("the seeded agents are missing");
    harness.hub.addUnread(atlas);
    harness.hub.setBusy(scout, true);
    const socket = await harness.openSocket("/api/hub/ws");
    await socket.next((f) => f.type === "agent_activity" && f.name === "scout");
    expect(await socket.quietFrames()).toEqual([]);
    expect(socket.frames.map((f) => f.type)).toEqual([
      "agents_snapshot",
      "agent_activity",
      "agent_activity",
    ]);
    // In the order the agents were created: scout, then atlas.
    expect(socket.frames.slice(1)).toEqual([
      { type: "agent_activity", name: "scout", busy: true, unread: 0 },
      { type: "agent_activity", name: "atlas", busy: false, unread: 1 },
    ]);
  });

  it("starts empty before setup has created an agent", async () => {
    harness = await startMockServer({ seed: false });
    const socket = await harness.openSocket("/api/hub/ws");
    expect(await socket.next()).toEqual({ type: "agents_snapshot", agents: [] });
  });

  it("tells every page when an agent's activity changes", async () => {
    harness = await startMockServer();
    const first = await harness.openSocket("/api/hub/ws");
    const second = await harness.openSocket("/api/hub/ws");
    const atlas = harness.hub.agents.get("atlas");
    if (!atlas) throw new Error("the seeded agents are missing");
    await first.nextOfType("agents_snapshot");
    await second.nextOfType("agents_snapshot");
    harness.hub.setBusy(atlas, true);
    const expected = { type: "agent_activity", name: "atlas", busy: true, unread: 0 };
    expect(await first.nextOfType("agent_activity")).toEqual(expected);
    expect(await second.nextOfType("agent_activity")).toEqual(expected);
  });

  it("clears an agent's unread count when a page opens its socket", async () => {
    harness = await startMockServer();
    const atlas = harness.hub.agents.get("atlas");
    if (!atlas) throw new Error("the seeded agents are missing");
    harness.hub.addUnread(atlas);
    const socket = await harness.openSocket("/api/hub/ws");
    await socket.next((f) => f.type === "agent_activity" && f.unread === 1);
    await harness.openSocket("/api/agents/atlas/ws");
    expect(await socket.next((f) => f.type === "agent_activity" && f.unread === 0)).toEqual({
      type: "agent_activity",
      name: "atlas",
      busy: false,
      unread: 0,
    });
    expect(atlas.unread).toBe(0);
  });

  describe("watch_team", () => {
    let socket: TestSocket;

    beforeEach(async () => {
      harness = await startMockServer();
      socket = await harness.openSocket("/api/hub/ws");
      await socket.nextOfType("agents_snapshot");
    });

    /** What the hub said in answer to a frame the page sent. */
    async function answer(frame: object | string): Promise<Frame[]> {
      if (typeof frame === "string") socket.sendRaw(frame);
      else socket.send(frame);
      return socket.quietFrames();
    }

    async function refusal(frame: object | string): Promise<Frame | undefined> {
      return (await answer(frame)).find((f) => f.type === "notice");
    }

    it("accepts team paths quietly", async () => {
      expect(await answer({ type: "watch_team", prefixes: ["team", "team/wiki"] })).toEqual([]);
      expect(await answer({ type: "watch_team", prefixes: [] })).toEqual([]);
    });

    it.each([
      [
        { type: "watch_team", prefixes: ["wiki"] },
        'Couldn\'t watch "wiki": team watch paths start with team/, like "team/wiki".',
      ],
      [
        { type: "watch_team", prefixes: ["team", "teamwork"] },
        'Couldn\'t watch "teamwork": team watch paths start with team/, like "team/wiki".',
      ],
      [
        { type: "watch_team", prefixes: [""] },
        'Couldn\'t watch "": team watch paths start with team/, like "team/wiki".',
      ],
      [
        { type: "watch_team", prefixes: ["team/../memory"] },
        'Couldn\'t watch the team files: can\'t watch "team/../memory": watch paths must stay inside the workspace (no "..").',
      ],
    ])("refuses %j with a warning", async (frame, message) => {
      expect(await refusal(frame)).toEqual({ type: "notice", level: "warn", message });
    });

    it.each([
      "not json",
      JSON.stringify({ type: "something_else" }),
      JSON.stringify({ type: "watch_team" }),
      JSON.stringify({ type: "watch_team", prefixes: [1] }),
      JSON.stringify([]),
    ])("doesn't understand %s", async (frame) => {
      expect(await refusal(frame)).toEqual({
        type: "notice",
        level: "warn",
        message:
          "Residuum couldn't read a message from this page. Reload the page if team files stop updating.",
      });
    });
  });
});
