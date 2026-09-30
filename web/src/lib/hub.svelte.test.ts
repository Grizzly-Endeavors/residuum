import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { HubStore } from "./hub.svelte";
import { notifications } from "./notifications.svelte";
import { toast } from "./toast.svelte";
import type { AgentSummary, DeletedAgent } from "./hub-types";

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

  it("does not repeat the agent's name when the notice already starts with it", () => {
    const hub = new HubStore();
    hub.handleFrame({
      type: "notice",
      level: "error",
      message: "scout failed: no key",
      agent: "scout",
    });
    expect(notifications.history.map((n) => n.message)).toEqual(["scout failed: no key"]);
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
  /** A hub whose socket is open. */
  function openHub(): HubStore {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(jsonResponse({ agents: [] }))),
    );
    const hub = new HubStore();
    hub.connect();
    FakeWebSocket.last.simulateOpen();
    return hub;
  }

  const nothing = { changed: () => {} };

  it("sends the union of its owners' prefixes on the hub socket, and only when it changes", () => {
    const hub = openHub();
    const wiki = hub.teamWatches.register(nothing);
    const chart = hub.teamWatches.register(nothing);

    wiki.set(["team/wiki"]);
    chart.set(["team/workbench/chart", "team/wiki"]);
    chart.set(["team/wiki", "team/workbench/chart"]);
    wiki.release();
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team/wiki"] },
      { type: "watch_team", prefixes: ["team/wiki", "team/workbench/chart"] },
    ]);
    chart.release();
    expect(FakeWebSocket.last.sentFrames().at(-1)).toEqual({ type: "watch_team", prefixes: [] });
    hub.disconnect();
  });

  it("spells prefixes as the hub's change feed does: team or team/...", () => {
    const hub = openHub();
    hub.teamWatches.register(nothing).set(["team", "team//workbench/./chart/"]);
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team", "team/workbench/chart"] },
    ]);
    hub.disconnect();
  });

  it.each(["wiki", "workbench/chart", "teams/wiki", "team/../secrets", "/team/wiki", ""])(
    "refuses the prefix %j, which is not under team/, and keeps the current watch",
    (prefix) => {
      const hub = openHub();
      const owner = hub.teamWatches.register(nothing);
      owner.set(["team/wiki"]);

      expect(() => {
        owner.set(["team/notes", prefix]);
      }).toThrow(TypeError);
      expect(FakeWebSocket.last.sentFrames()).toEqual([
        { type: "watch_team", prefixes: ["team/wiki"] },
      ]);
      hub.disconnect();
    },
  );

  it("sends the union again on a new connection, which starts watching nothing", () => {
    vi.useFakeTimers();
    const hub = openHub();
    hub.teamWatches.register(nothing).set(["team/wiki"]);
    hub.teamWatches.register(nothing).set(["team/notes"]);

    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(1000);
    FakeWebSocket.last.simulateOpen();
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team/notes", "team/wiki"] },
    ]);
    hub.disconnect();
  });

  it("sends nothing on a new connection when it watches nothing", () => {
    vi.useFakeTimers();
    const hub = openHub();
    FakeWebSocket.last.simulateClose();
    vi.advanceTimersByTime(1000);
    FakeWebSocket.last.simulateOpen();
    expect(FakeWebSocket.last.sent).toEqual([]);
    hub.disconnect();
  });

  it("holds a watch registered before the socket opens until it does", () => {
    const hub = new HubStore();
    hub.teamWatches.register(nothing).set(["team/wiki"]);
    hub.connect();
    expect(FakeWebSocket.last.sent).toEqual([]);
    FakeWebSocket.last.simulateOpen();
    expect(FakeWebSocket.last.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team/wiki"] },
    ]);
    hub.disconnect();
  });

  it("hands each owner the team changes under its prefixes, and every owner a resync", () => {
    const hub = openHub();
    const wiki: string[][] = [];
    const chart: string[][] = [];
    const resyncs: string[] = [];
    hub.teamWatches
      .register({
        changed: (changes) => wiki.push(changes.map((c) => c.path)),
        resync: (reason) => resyncs.push(`wiki:${reason}`),
      })
      .set(["team/wiki"]);
    hub.teamWatches
      .register({
        changed: (changes) => chart.push(changes.map((c) => c.path)),
        resync: (reason) => resyncs.push(`chart:${reason}`),
      })
      .set(["team/workbench/chart"]);

    hub.handleFrame({
      type: "workspace_changed",
      changes: [
        { path: "team/wiki/a.md", kind: "modified" },
        { path: "team/workbench/chart/index.html", kind: "created" },
        { path: "team/wikipedia/b.md", kind: "modified" },
      ],
    });
    hub.handleFrame({ type: "workspace_resync", reason: "overflow" });

    expect(wiki).toEqual([["team/wiki/a.md"]]);
    expect(chart).toEqual([["team/workbench/chart/index.html"]]);
    expect(resyncs).toEqual(["wiki:overflow", "chart:overflow"]);
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

const gone = (name: string): DeletedAgent => ({
  name,
  deleted_at: "2026-09-29T10:00:00Z",
  checkpoint_id: `ckpt-${name}`,
});

describe("HubStore deleted agents", () => {
  it("loads the deleted list and reports a failed load where the view can show it", async () => {
    const fetchMock = vi.fn((_url: string) =>
      Promise.resolve(jsonResponse({ agents: [gone("nova")] })),
    );
    vi.stubGlobal("fetch", fetchMock);
    const hub = new HubStore();

    await hub.refreshDeleted();

    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/hub/agents/deleted");
    expect(hub.deleted.map((d) => d.name)).toEqual(["nova"]);
    expect(hub.deletedLoaded).toBe(true);
    expect(hub.deletedError).toBeNull();

    vi.stubGlobal("fetch", () => Promise.resolve(jsonResponse({ error: "unreadable" }, 500)));
    await hub.refreshDeleted();

    expect(hub.deletedError).toContain("Couldn't load the deleted agents");
    expect(hub.deleted.map((d) => d.name)).toEqual(["nova"]);
    expect(notifications.history).toEqual([]);
  });

  it("refetches the list after a deletion only once the list has been asked for", async () => {
    const fetchMock = vi.fn((_url: string) => Promise.resolve(jsonResponse({ agents: [] })));
    vi.stubGlobal("fetch", fetchMock);
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("nova")] });

    hub.handleFrame({ type: "agent_deleted", name: "nova", by: "user" });
    expect(fetchMock).not.toHaveBeenCalled();

    await hub.refreshDeleted();
    fetchMock.mockClear();
    hub.handleFrame({ type: "agent_deleted", name: "nova", by: "agent:scout" });
    await vi.waitFor(() => {
      expect(fetchMock.mock.calls.map((call) => call[0])).toEqual(["/api/hub/agents/deleted"]);
    });
  });

  it("puts an Undo on the deleted toast that restores the agent by name", async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(jsonResponse(agent("nova"), 201)),
    );
    vi.stubGlobal("fetch", fetchMock);
    const hub = new HubStore();
    hub.handleFrame({ type: "agents_snapshot", agents: [agent("nova")] });

    hub.handleFrame({ type: "agent_deleted", name: "nova", by: "agent:scout" });

    const deletedToast = [...toast.toasts.values()].find(
      (t) => t.message === "scout deleted nova.",
    );
    expect(deletedToast?.action?.label).toBe("Undo");
    toast.runAction(deletedToast?.id ?? -1);
    await vi.waitFor(() => {
      expect(hub.agent("nova")?.state).toBe("running");
    });
    expect(fetchMock.mock.calls[0]?.[0]).toBe("/api/hub/agents/restore");
    expect(fetchMock.mock.calls[0]?.[1]?.method).toBe("POST");
    expect(fetchMock.mock.calls[0]?.[1]?.body).toBe(JSON.stringify({ name: "nova" }));
  });

  it("restores an agent, sending the checkpoint when given one, and drops it from the deleted list", async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(jsonResponse(agent("nova"), 201)),
    );
    vi.stubGlobal("fetch", fetchMock);
    const hub = new HubStore();
    hub.deleted = [gone("nova"), gone("kit")];

    const restored = await hub.restoreAgent("nova", "ckpt-nova");

    expect(restored?.name).toBe("nova");
    expect(fetchMock.mock.calls[0]?.[1]?.body).toBe(
      JSON.stringify({ name: "nova", checkpoint_id: "ckpt-nova" }),
    );
    expect(hub.agents.map((a) => a.name)).toEqual(["nova"]);
    expect(hub.deleted.map((d) => d.name)).toEqual(["kit"]);
  });

  it("tells the user why a restore failed and leaves the lists alone", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(jsonResponse({ error: "an agent named 'nova' already exists" }, 409)),
    );
    const hub = new HubStore();
    hub.deleted = [gone("nova")];

    expect(await hub.restoreAgent("nova")).toBeNull();

    expect(hub.agents).toEqual([]);
    expect(hub.deleted.map((d) => d.name)).toEqual(["nova"]);
    expect(notifications.history[0]?.kind).toBe("error");
    expect(notifications.history[0]?.message).toContain("Couldn't restore nova");
    expect(notifications.history[0]?.message).toContain("already exists");
  });

  it("explains a 404 as nothing to restore", async () => {
    vi.stubGlobal("fetch", () => Promise.resolve(jsonResponse({ error: "no deleted agent" }, 404)));
    const hub = new HubStore();

    expect(await hub.restoreAgent("ghost")).toBeNull();

    expect(notifications.history[0]?.message).toContain("nothing to restore");
  });

  it("adds a restored agent from its frame, naming who restored it, and unlists it", () => {
    const hub = new HubStore();
    hub.deleted = [gone("nova"), gone("kit")];

    hub.handleFrame({ type: "agent_restored", agent: agent("nova"), by: "agent:scout" });

    expect(hub.agents.map((a) => a.name)).toEqual(["nova"]);
    expect(hub.deleted.map((d) => d.name)).toEqual(["kit"]);
    expect(hub.notices[0]?.message).toBe("scout restored nova.");
  });

  it("never lists an agent that was created again under a deleted name", () => {
    const hub = new HubStore();
    hub.deleted = [gone("nova")];

    hub.handleFrame({ type: "agent_created", agent: agent("nova"), by: "user" });

    expect(hub.deleted).toEqual([]);
  });
});
