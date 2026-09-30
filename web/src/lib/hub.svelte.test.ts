import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { HubStore } from "./hub.svelte";
import { notifications } from "./notifications.svelte";
import { toast } from "./toast.svelte";
import type { AgentSummary } from "./hub-types";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    ...overrides,
  };
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

beforeEach(() => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

const newAgent = {
  name: "atlas",
  description: null,
  models_from: "scout",
  providers_toml: null,
  a2a_visibility: null,
};

describe("HubStore frames", () => {
  it("replaces the agent list from a snapshot, sorted by name", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout"), agent("atlas")] });
    expect(hub.agents.map((a) => a.name)).toEqual(["atlas", "scout"]);
    expect(hub.loaded).toBe(true);
  });

  it("raises one error notice when an agent moves into failed, and none for a snapshot", () => {
    const hub = new HubStore();
    const failed = agent("atlas", {
      state: "failed",
      last_error: { message: "bad providers.toml", at: "2026-01-01T00:00:00Z" },
    });
    hub.handleFrame({ type: "agents_snapshot", agents: [failed] });
    expect(hub.notices).toEqual([]);

    hub.handleFrame({ type: "agent_state", agent: agent("atlas", { state: "stopped" }) });
    hub.handleFrame({ type: "agent_state", agent: failed });
    hub.handleFrame({ type: "agent_state", agent: failed });
    expect(hub.notices.map((n) => [n.level, n.message, n.agent])).toEqual([
      ["error", "atlas failed: bad providers.toml", "atlas"],
    ]);
  });

  it("drops the activity of agents a later snapshot no longer lists", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("atlas"), agent("scout")] });
    hub.handleFrame({ type: "agent_activity", name: "atlas", busy: true, unread: 2 });
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });
    expect(hub.activityOf("atlas")).toEqual({ busy: false, unread: 0 });
  });

  it("updates an agent's state in place", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("atlas"), agent("scout")] });
    hub.handleFrame({
      type: "agent_state",
      agent: agent("scout", {
        state: "failed",
        last_error: { message: "bad config", at: "2026-09-29T12:00:00Z" },
      }),
    });
    expect(hub.agents.map((a) => a.name)).toEqual(["atlas", "scout"]);
    expect(hub.agent("scout")?.state).toBe("failed");
    expect(hub.agent("scout")?.last_error?.message).toBe("bad config");
  });

  it("adds a created agent and removes a deleted one with its activity", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });
    hub.handleFrame({ type: "agent_created", agent: agent("atlas"), by: "agent:scout" });
    hub.handleFrame({ type: "agent_activity", name: "atlas", busy: true, unread: 1 });
    expect(hub.agents.map((a) => a.name)).toEqual(["atlas", "scout"]);

    hub.handleFrame({ type: "agent_deleted", name: "atlas", by: "user" });
    expect(hub.agents.map((a) => a.name)).toEqual(["scout"]);
    expect(hub.activityOf("atlas")).toEqual({ busy: false, unread: 0 });
  });

  it("tracks busy and unread per agent", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "agent_activity", name: "scout", busy: true, unread: 0 });
    hub.handleFrame({ type: "agent_activity", name: "atlas", busy: false, unread: 3 });
    hub.handleFrame({ type: "agent_activity", name: "scout", busy: false, unread: 1 });
    expect(hub.activityOf("scout")).toEqual({ busy: false, unread: 1 });
    expect(hub.activityOf("atlas")).toEqual({ busy: false, unread: 3 });
    expect(hub.activityOf("nobody")).toEqual({ busy: false, unread: 0 });
  });

  it("keeps notices and shows each as a toast, naming the agent it concerns", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "notice", level: "warn", message: "config reloaded" });
    hub.handleFrame({ type: "notice", level: "error", message: "it failed", agent: "scout" });

    expect(hub.notices.map((n) => [n.level, n.message, n.agent])).toEqual([
      ["error", "it failed", "scout"],
      ["warn", "config reloaded", undefined],
    ]);
    expect(notifications.history.map((n) => [n.kind, n.message])).toEqual([
      ["error", "scout: it failed"],
      ["notice", "config reloaded"],
    ]);
    expect([...toast.toasts.values()].map((t) => t.kind)).toEqual(["info", "error"]);
  });

  it("toasts a created agent naming who created it", () => {
    const hub = new HubStore();
    hub.handleFrame({ type: "agent_created", agent: agent("atlas"), by: "user" });
    hub.handleFrame({ type: "agent_created", agent: agent("nova"), by: "agent:scout" });
    hub.handleFrame({ type: "agent_deleted", name: "atlas", by: "agent:nova" });
    hub.handleFrame({ type: "agent_deleted", name: "nova", by: "user" });

    expect(hub.agents).toHaveLength(0);
    expect(hub.notices.map((n) => n.message).reverse()).toEqual([
      "You created atlas.",
      "scout created nova.",
      "nova deleted atlas.",
      "You deleted nova.",
    ]);
    expect([...toast.toasts.values()]).toHaveLength(4);
  });

  it("caps the notices it keeps", () => {
    const hub = new HubStore();
    for (let i = 0; i < 60; i++) {
      hub.handleFrame({ type: "notice", level: "info", message: `n${i}` });
    }
    expect(hub.notices).toHaveLength(50);
    expect(hub.notices[0]?.message).toBe("n59");
  });

  it("hands team changes to listeners until they stop listening", () => {
    const hub = new HubStore();
    const seen: string[][] = [];
    const stop = hub.onTeamChange((changes) => seen.push(changes.map((c) => c.path)));
    const change = { path: "team/wiki/a.md", kind: "modified" } as const;
    hub.handleFrame({ type: "workspace_changed", changes: [change] });
    stop();
    hub.handleFrame({ type: "workspace_changed", changes: [change] });
    expect(seen).toEqual([["team/wiki/a.md"]]);
  });

  it("tells frame observers after the store has handled the frame", () => {
    const hub = new HubStore();
    let listedWhenSeen: string[] = [];
    hub.onFrame(() => {
      listedWhenSeen = hub.agents.map((a) => a.name);
    });
    hub.handleFrame({ type: "agent_created", agent: agent("atlas"), by: "user" });
    expect(listedWhenSeen).toEqual(["atlas"]);
  });

  it("handles frames arriving over the socket", () => {
    const hub = new HubStore();
    hub.connect();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    FakeWebSocket.last.simulateOpen();
    FakeWebSocket.last.simulateMessage({ type: "agents_snapshot", agents: [agent("scout")] });
    expect(hub.agents.map((a) => a.name)).toEqual(["scout"]);
    hub.disconnect();
  });
});

describe("HubStore connection", () => {
  it("connects to the hub socket", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    expect(FakeWebSocket.last.url).toBe("ws://localhost:7700/api/hub/ws");
    hub.disconnect();
  });

  it("sends no pings: the hub socket takes only watch_team", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();
    vi.advanceTimersByTime(120_000);
    expect(FakeWebSocket.last.sent).toEqual([]);
    hub.disconnect();
  });

  it("reconnects with a growing delay and resets it once connected", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    expect(FakeWebSocket.sockets).toHaveLength(1);

    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(999);
    expect(FakeWebSocket.sockets).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeWebSocket.sockets).toHaveLength(2);

    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(1499);
    expect(FakeWebSocket.sockets).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(FakeWebSocket.sockets).toHaveLength(3);

    FakeWebSocket.last.simulateOpen();
    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(1000);
    expect(FakeWebSocket.sockets).toHaveLength(4);
    hub.disconnect();
  });

  it("stops reconnecting once disconnected", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateClose();
    hub.disconnect();
    vi.advanceTimersByTime(60_000);
    expect(FakeWebSocket.sockets).toHaveLength(1);
  });

  it("keeps the agent list across a reconnect and replaces it from the new snapshot", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();
    FakeWebSocket.last.simulateMessage({ type: "agents_snapshot", agents: [agent("scout")] });
    FakeWebSocket.last.simulateClose();
    expect(hub.agents.map((a) => a.name)).toEqual(["scout"]);

    vi.advanceTimersByTime(1000);
    FakeWebSocket.last.simulateOpen();
    FakeWebSocket.last.simulateMessage({
      type: "agents_snapshot",
      agents: [agent("atlas"), agent("scout")],
    });
    expect(hub.agents.map((a) => a.name)).toEqual(["atlas", "scout"]);
    hub.disconnect();
  });
});

describe("HubStore team watching", () => {
  it("sends the watch set and only when it changes", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();

    hub.watchTeam(["team/wiki", "team/workbench/chart"]);
    hub.watchTeam(["team/workbench/chart", "team/wiki"]);
    hub.watchTeam(["team/wiki"]);
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team/wiki", "team/workbench/chart"] },
      { type: "watch_team", prefixes: ["team/wiki"] },
    ]);
    hub.disconnect();
  });

  it("spells prefixes as the hub's change feed does: team or team/...", () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();

    hub.watchTeam(["team", "team//workbench/./chart/"]);
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team", "team/workbench/chart"] },
    ]);
    hub.disconnect();
  });

  it.each(["wiki", "workbench/chart", "teams/wiki", "team/../secrets", "/team/wiki", ""])(
    "refuses the prefix %j, which is not under team/, and keeps the current watch",
    (prefix) => {
      vi.stubGlobal(
        "fetch",
        vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
      );
      const hub = new HubStore();
      hub.connect();
      FakeWebSocket.last.simulateOpen();
      hub.watchTeam(["team/wiki"]);

      expect(() => {
        hub.watchTeam(["team/notes", prefix]);
      }).toThrow(TypeError);
      expect(FakeWebSocket.last.sentFrames()).toEqual([
        { type: "watch_team", prefixes: ["team/wiki"] },
      ]);
      hub.disconnect();
    },
  );

  it("sends the watch set again on a new connection, which starts watching nothing", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();
    hub.watchTeam(["team/wiki"]);

    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(1000);
    FakeWebSocket.last.simulateOpen();
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team/wiki"] },
    ]);
    hub.disconnect();
  });

  it("sends nothing on a new connection when it watches nothing", () => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();
    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(1000);
    FakeWebSocket.last.simulateOpen();
    expect(FakeWebSocket.last.sent).toEqual([]);
    hub.disconnect();
  });
});

describe("HubStore list fetch", () => {
  it("loads the list over HTTP", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [agent("scout"), agent("atlas")] }))),
    );
    const hub = new HubStore();
    await hub.refresh();
    expect(hub.agents.map((a) => a.name)).toEqual(["atlas", "scout"]);
    expect(hub.loaded).toBe(true);
  });

  it("ignores a fetch that lands after the socket's snapshot, unless forced", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [agent("stale")] }))),
    );
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });

    await hub.refresh();
    expect(hub.agents.map((a) => a.name)).toEqual(["scout"]);

    await hub.refresh(true);
    expect(hub.agents.map((a) => a.name)).toEqual(["stale"]);
  });
});

describe("HubStore lifecycle actions", () => {
  it("folds the returned summary into the list", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse(agent("scout", { state: "stopped" })))),
    );
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });

    expect(await hub.stopAgent("scout")).toBe(true);
    expect(hub.agent("scout")?.state).toBe("stopped");
  });

  it("adds a created agent and returns it", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse(agent("atlas"), 201))),
    );
    const hub = new HubStore();
    const created = await hub.createAgent(newAgent);
    expect(created?.name).toBe("atlas");
    expect(hub.agents.map((a) => a.name)).toEqual(["atlas"]);
  });

  it("removes a deleted agent", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ deleted: true, checkpoint_id: "c1" }))),
    );
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });
    expect((await hub.deleteAgent("scout"))?.checkpoint_id).toBe("c1");
    expect(hub.agents).toEqual([]);
  });

  it("tells the user when an action fails and leaves the list alone", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ error: "an agent named 'atlas' exists" }, 409))),
    );
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });

    expect(await hub.createAgent(newAgent)).toBeNull();
    expect(hub.agents.map((a) => a.name)).toEqual(["scout"]);
    expect(notifications.history).toHaveLength(1);
    expect(notifications.history[0]?.kind).toBe("error");
  });

  it("sends the autostart change", async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(jsonResponse(agent("scout", { autostart: false }))),
    );
    vi.stubGlobal("fetch", fetchMock);
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });

    await hub.setAutostart("scout", false);
    expect(fetchMock.mock.calls[0]?.[1]?.body).toBe(JSON.stringify({ autostart: false }));
    expect(hub.agent("scout")?.autostart).toBe(false);
  });

  it("sends the visibility change to the agent's route", async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(jsonResponse(agent("scout", { a2a_visibility: "public" }))),
    );
    vi.stubGlobal("fetch", fetchMock);
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });

    expect(await hub.setVisibility("scout", "public")).toBe(true);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/hub/agents/scout");
    expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("PATCH");
    expect(fetchMock.mock.calls[0]?.[1]?.body).toBe(JSON.stringify({ a2a_visibility: "public" }));
    expect(hub.agent("scout")?.a2a_visibility).toBe("public");
  });

  it("reports a failed visibility change and leaves the agent as it was", async () => {
    vi.stubGlobal("fetch", () => Promise.resolve(jsonResponse({ error: "nope" }, 500)));
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("scout")] });

    expect(await hub.setVisibility("scout", "public")).toBe(false);
    expect(hub.agent("scout")?.a2a_visibility).toBe("private");
    expect(notifications.history[0]?.kind).toBe("error");
  });
});
