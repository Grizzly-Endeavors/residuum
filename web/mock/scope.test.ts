import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { isRefusal, isFileDataRoute, scopeRequest } from "./scope";
import {
  fetchJson,
  fetchText,
  startMockServer,
  createStubHub,
  type MockServerHarness,
} from "./test-support";

describe("isFileDataRoute", () => {
  // The same paths the backend's dispatch tests check (`src/hub/http/dispatch.rs`).
  it.each([
    "/chat/history",
    "/usage",
    "/a2a/agents/raw",
    "/inbox",
    "/inbox/archive",
    "/inbox/2026-09-30-note/read",
    "/inbox/2026-09-30-note/archive",
    "/inbox/2026-09-30-note/restore",
    "/inbox/2026-09-30-note/attachments/0",
    "/inbox/archive/read",
  ])("takes %s", (route) => {
    expect(isFileDataRoute(route)).toBe(true);
  });

  it.each([
    "/status",
    "/sessions",
    "/configuration",
    "/workspaces",
    "/files/workspace",
    "/agent-inbox",
    "/chat",
    "/chat/history/extra",
    "/usage/totals",
    "/a2a/agents",
    "/a2a/agents/raw/extra",
    "/a2a/status",
    "/a2a/card",
    "/a2a/outbound",
    "/inbox/item",
    "/inbox/item/unknown",
    "/inbox/item/attachments",
    "/inbox/item/attachments/0/extra",
    "/inbox/",
    "",
  ])("leaves %s to a running agent", (route) => {
    expect(isFileDataRoute(route)).toBe(false);
  });
});

describe("scopeRequest", () => {
  const query = new URLSearchParams();

  function setup(): ReturnType<typeof createStubHub> {
    const hub = createStubHub();
    hub.createAgent("atlas");
    hub.createAgent("drifter", { runState: "stopped" });
    return hub;
  }

  it("runs an agent's routes against its own state, unscoped", () => {
    const hub = setup();
    const scoped = scopeRequest(hub, "/api/agents/atlas/chat/history", query);
    expect(scoped).toEqual({ state: hub.agents.get("atlas")?.state, path: "/api/chat/history" });
  });

  it("answers 404 for an agent that doesn't exist", () => {
    const scoped = scopeRequest(setup(), "/api/agents/ghost/status", query);
    expect(scoped).toEqual({ status: 404, body: { error: "no agent named 'ghost'" } });
  });

  it("refuses a live route on a stopped agent with its state", () => {
    const scoped = scopeRequest(setup(), "/api/agents/drifter/status", query);
    expect(isRefusal(scoped) && scoped).toEqual({
      status: 409,
      body: { error: "drifter is stopped", state: "stopped" },
    });
  });

  it.each(["/config/raw", "/workspace/file", "/chat/history", "/inbox", "/a2a/agents/raw"])(
    "lets a stopped agent serve %s",
    (route) => {
      const scoped = scopeRequest(setup(), `/api/agents/drifter${route}`, query);
      expect(isRefusal(scoped)).toBe(false);
    },
  );

  it("keeps the hub's own routes scoped to the shared state", () => {
    const hub = setup();
    for (const path of [
      "/api/hub/agents",
      "/api/hub/agents/atlas/start",
      "/api/hub/status",
      "/api/hub/inbox",
      "/api/hub/inbox/unread",
      "/api/hub/inbox/atlas/note/read",
      "/api/hub/config/raw",
      "/api/team/workspace/files",
    ]) {
      expect(scopeRequest(hub, path, query)).toEqual({ state: hub.hubState, path });
    }
  });

  it("rewrites other hub and team routes to their unscoped spelling", () => {
    const hub = setup();
    expect(scopeRequest(hub, "/api/hub/secrets", query)).toEqual({
      state: hub.hubState,
      path: "/api/secrets",
    });
    expect(scopeRequest(hub, "/api/team/workbench/info", query)).toEqual({
      state: hub.hubState,
      path: "/api/workbench/info",
    });
  });

  it("points a test control at the named agent, else the first running one, else the hub", () => {
    const hub = setup();
    const named = scopeRequest(hub, "/api/mock/x", new URLSearchParams("agent=drifter"));
    expect(named).toMatchObject({ state: hub.agents.get("drifter")?.state });
    const first = scopeRequest(hub, "/api/mock/x", query);
    expect(first).toMatchObject({ state: hub.agents.get("atlas")?.state });
    expect(scopeRequest(createStubHub(), "/api/mock/x", query)).toMatchObject({
      path: "/api/mock/x",
    });
  });

  it("refuses an /api path the contract doesn't scope", () => {
    const scoped = scopeRequest(setup(), "/api/status", query);
    expect(isRefusal(scoped) && scoped.status).toBe(404);
  });
});

describe("what each scope owns", () => {
  const query = new URLSearchParams();
  const hub = createStubHub();
  hub.createAgent("atlas");
  hub.createAgent("drifter", { runState: "stopped" });

  it.each([
    "status",
    "config/raw",
    "providers/models",
    "mcp/patch",
    "workspace/tree",
    "checkpoints/stats",
    "chat/history",
    "usage",
    "inbox/archive",
    "agent-inbox",
    "sessions/runs/x/transcript",
    "scheduled/pulses",
    "model/complete",
    "a2a/status",
    "a2a/card",
    "a2a/agents/raw",
    "a2a/outbound/t/stop",
  ])("gives an agent /%s", (route) => {
    const scoped = scopeRequest(hub, `/api/agents/atlas/${route}`, query);
    expect(isRefusal(scoped)).toBe(false);
  });

  it.each([
    "workbench/artifacts",
    "workbench/info",
    "secrets",
    "agent-keys",
    "a2a/keys",
    "cloud/status",
    "update/status",
    "tracing/feedback",
    "system/timezone",
    "mcp-catalog",
    "agents",
    "inbox-not-really",
  ])("answers 404 for an agent's /%s, which isn't its own", (route) => {
    const scoped = scopeRequest(hub, `/api/agents/atlas/${route}`, query);
    expect(isRefusal(scoped) && scoped.status).toBe(404);
  });

  it.each([
    "secrets",
    "agent-keys",
    "a2a/keys",
    "cloud/status",
    "update/check",
    "tracing/status",
    "system/timezone",
    "mcp-catalog",
    "providers/models",
    "checkpoints/stats",
  ])("gives the hub /%s", (route) => {
    expect(scopeRequest(hub, `/api/hub/${route}`, query)).toEqual({
      state: hub.hubState,
      path: `/api/${route}`,
    });
  });

  it.each([
    "workbench/artifacts",
    "workspace/files",
    "sessions",
    "chat/history",
    "status/x",
    "inbox-not-really",
  ])("answers 404 for the hub's /%s", (route) => {
    const scoped = scopeRequest(hub, `/api/hub/${route}`, query);
    expect(isRefusal(scoped) && scoped.status).toBe(404);
  });

  it.each(["workbench/artifacts", "workbench/info", "workbench/artifacts/x"])(
    "gives the team /%s",
    (route) => {
      expect(scopeRequest(hub, `/api/team/${route}`, query)).toEqual({
        state: hub.hubState,
        path: `/api/${route}`,
      });
    },
  );

  it.each(["secrets", "sessions", "checkpoints", "status", "wiki"])(
    "answers 404 for the team's /%s",
    (route) => {
      const scoped = scopeRequest(hub, `/api/team/${route}`, query);
      expect(isRefusal(scoped) && scoped.status).toBe(404);
    },
  );

  it("names the scope that doesn't own the route", () => {
    const scoped = scopeRequest(hub, "/api/agents/atlas/workbench/artifacts", query);
    expect(isRefusal(scoped) && scoped.body.error).toBe(
      "mock: /api/agents/atlas/workbench/artifacts is not a route of an agent",
    );
  });

  it("answers a stopped agent's route it doesn't own with 409, as the backend does before its router runs", () => {
    const scoped = scopeRequest(hub, "/api/agents/drifter/workbench/artifacts", query);
    expect(isRefusal(scoped) && scoped.status).toBe(409);
  });
});

describe("stopped and failed agents over HTTP", () => {
  let harness: MockServerHarness;

  beforeEach(async () => {
    harness = await startMockServer();
  });

  afterEach(async () => {
    await harness.close();
  });

  const at = (agent: string, route: string): string =>
    `${harness.baseUrl}/api/agents/${agent}${route}`;

  it.each(["drifter", "brittle"])("serves %s's file-only routes, with no data", async (agent) => {
    expect(await fetchJson(at(agent, "/chat/history"))).toEqual({
      status: 200,
      body: { kind: "recent", messages: [], next_cursor: null },
    });
    expect(await fetchJson(at(agent, "/chat/history?episode=ep-003"))).toEqual({
      status: 404,
      body: { error: "episode not found" },
    });
    expect(await fetchJson(at(agent, "/usage"))).toEqual({
      status: 200,
      body: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    });
    expect(await fetchJson(at(agent, "/inbox"))).toEqual({ status: 200, body: [] });
    expect(await fetchText(at(agent, "/a2a/agents/raw"))).toEqual({
      status: 200,
      body: '{"agents":{}}',
    });
  });

  it("round-trips a stopped agent's raw A2A settings", async () => {
    const settings = '{"agents":{"peer":{"url":"https://example.com/a2a/peer"}}}';
    const put = await fetchJson(at("drifter", "/a2a/agents/raw"), {
      method: "PUT",
      body: settings,
    });
    expect(put).toEqual({ status: 200, body: { valid: true } });
    expect((await fetchText(at("drifter", "/a2a/agents/raw"))).body).toBe(settings);
    expect((await fetchText(at("brittle", "/a2a/agents/raw"))).body).toBe('{"agents":{}}');
  });

  it("keeps the repair routes working", async () => {
    const config = await fetchText(at("drifter", "/config/raw"));
    expect(config.status).toBe(200);
    expect(config.body).toContain("[");
  });

  it.each([
    ["drifter", "stopped"],
    ["brittle", "failed"],
  ])("still answers 409 for %s's live routes", async (agent, state) => {
    for (const route of ["/status", "/sessions", "/a2a/status", "/inbox/mock_1"]) {
      expect(await fetchJson(at(agent, route))).toEqual({
        status: 409,
        body: { error: `${agent} is ${state}`, state },
      });
    }
  });

  it("answers 404 for an agent that doesn't exist", async () => {
    expect(await fetchJson(at("ghost", "/chat/history"))).toEqual({
      status: 404,
      body: { error: "no agent named 'ghost'" },
    });
  });

  it("serves the inbox handlers the stopped agent's own state", async () => {
    const atlas = await fetchJson(at("atlas", "/inbox"));
    expect((atlas.body as unknown[]).length).toBeGreaterThan(0);
    await fetchJson(`${harness.baseUrl}/api/hub/agents/atlas/stop`, { method: "POST" });
    expect(await fetchJson(at("atlas", "/inbox"))).toEqual(atlas);
  });

  it("keeps a stopped agent's conversation readable", async () => {
    const before = (await fetchJson(at("atlas", "/chat/history"))).body as { messages: unknown[] };
    expect(before.messages.length).toBeGreaterThan(0);
    await fetchJson(`${harness.baseUrl}/api/hub/agents/atlas/stop`, { method: "POST" });
    const after = (await fetchJson(at("atlas", "/chat/history"))).body as { messages: unknown[] };
    expect(after.messages.length).toBe(before.messages.length);
    expect((await fetchJson(at("atlas", "/status"))).status).toBe(409);
  });

  it("gives an agent its conversation and inbox when it first runs", async () => {
    await fetchJson(`${harness.baseUrl}/api/hub/agents/drifter/start`, { method: "POST" });
    const history = (await fetchJson(at("drifter", "/chat/history"))).body as {
      messages: { content: string }[];
    };
    expect(history.messages.at(-1)?.content).toContain("Hi, this is drifter");
    const inbox = (await fetchJson(at("drifter", "/inbox"))).body as unknown[];
    expect(inbox.length).toBeGreaterThan(0);
  });

  it("refuses a failed start and keeps the agent without a conversation", async () => {
    const started = await fetchJson(`${harness.baseUrl}/api/hub/agents/brittle/start`, {
      method: "POST",
    });
    expect(started.body).toMatchObject({ state: "failed" });
    expect((await fetchJson(at("brittle", "/chat/history"))).body).toEqual({
      kind: "recent",
      messages: [],
      next_cursor: null,
    });
  });
});
