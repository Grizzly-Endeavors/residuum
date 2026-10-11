// The workbench SDK's live side (assets/workbench/sdk.js): the hub socket it
// opens as the page loads, live reload, `on` and `watch`, agent handles, and
// sessions followed through the hub's session relay.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { FakeWebSocket } from "../test/fake-websocket";
import {
  lastRequest,
  loadSdk,
  settle,
  thrownBy,
  described,
  type Frame,
  type LoadedSdk,
  type SessionHandle,
} from "../test/workbench-sdk";

const HUB = "/api/hub/ws";

afterEach(() => {
  vi.useRealTimers();
});

/** A page whose hub socket is open. */
function connected(artifact = "chart"): LoadedSdk & { hub: FakeWebSocket } {
  const loaded = loadSdk(artifact);
  const hub = loaded.socket(HUB);
  hub.simulateOpen();
  return { ...loaded, hub };
}

const changed = (...paths: string[]): Frame => ({
  type: "workspace_changed",
  changes: paths.map((path) => ({ path, kind: "modified" })),
});

describe("the hub socket", () => {
  it("opens as the page loads, on the page's own origin", () => {
    const { socket } = loadSdk();
    expect(socket(HUB).url).toBe("ws://localhost:7702/api/hub/ws");
  });

  it("tells connection handlers when it drops and returns, and reconnects", async () => {
    vi.useFakeTimers();
    const { sdk, hub, socketCount, console } = connected();
    const seen: Frame[] = [];
    sdk.on("connection", (frame) => seen.push(frame));
    await settle();
    expect(seen).toEqual([{ type: "connection", state: "connected" }]);

    hub.simulateClose();
    expect(seen.at(-1)).toEqual({ type: "connection", state: "disconnected" });
    expect(console.warn).toHaveBeenCalledWith(expect.stringContaining("reconnecting"));
    await vi.advanceTimersByTimeAsync(1000);
    expect(socketCount(HUB)).toBe(2);
  });

  it("reports a socket that never opens as disconnected once", async () => {
    vi.useFakeTimers();
    const { sdk, socket } = loadSdk();
    const seen: Frame[] = [];
    sdk.on("*", (frame) => seen.push(frame));
    socket(HUB).simulateClose();
    await vi.advanceTimersByTimeAsync(1000);
    socket(HUB).simulateClose();
    expect(seen).toEqual([{ type: "connection", state: "disconnected" }]);
  });
});

describe("residuum.on", () => {
  it("throws for an agent's frame type, pointing at agent(name).on", () => {
    const { sdk } = loadSdk();
    for (const type of ["turn_started", "session_started", "notice"]) {
      const thrown = thrownBy(() => sdk.on(type, () => {}));
      expect(thrown.name).toBe("TypeError");
      expect(thrown.message).toContain(`residuum.agent("<name>").on("${type}", handler)`);
    }
  });

  it("hears artifact events with no agent involved, '*' included", () => {
    const { sdk, hub } = connected();
    const removed: Frame[] = [];
    const all: Frame[] = [];
    sdk.on("artifact_removed", (frame) => removed.push(frame));
    sdk.on("*", (frame) => all.push(frame));
    hub.simulateMessage({ type: "artifact_removed", name: "chart" });
    hub.simulateMessage({ type: "agent_activity", name: "atlas", busy: true });
    expect(removed).toEqual([{ type: "artifact_removed", name: "chart" }]);
    expect(all).toEqual([{ type: "artifact_removed", name: "chart" }]);
  });

  it("stops calling a handler once it unsubscribes", () => {
    const { sdk, hub } = connected();
    const seen: Frame[] = [];
    const stop = sdk.on("artifact_removed", (frame) => seen.push(frame));
    stop();
    hub.simulateMessage({ type: "artifact_removed", name: "other" });
    expect(seen).toEqual([]);
  });
});

describe("live reload", () => {
  it("reloads the page when its own artifact changes", () => {
    const { hub, reload } = connected("chart");
    hub.simulateMessage({ type: "artifact_updated", name: "other" });
    hub.simulateMessage({ type: "artifact_removed", name: "chart" });
    expect(reload).not.toHaveBeenCalled();
    hub.simulateMessage({ type: "artifact_updated", name: "chart" });
    expect(reload).toHaveBeenCalledOnce();
  });

  it("leaves it to the page once the page handles artifact_updated", () => {
    const { sdk, hub, reload } = connected("chart");
    const seen: Frame[] = [];
    const stop = sdk.on("artifact_updated", (frame) => seen.push(frame));
    hub.simulateMessage({ type: "artifact_updated", name: "chart" });
    expect(seen).toEqual([{ type: "artifact_updated", name: "chart" }]);
    expect(reload).not.toHaveBeenCalled();

    stop();
    hub.simulateMessage({ type: "artifact_updated", name: "chart" });
    expect(reload).toHaveBeenCalledOnce();
  });

  it("still reloads for a page that only listens to '*'", () => {
    const { sdk, hub, reload } = connected("chart");
    sdk.on("*", () => {});
    hub.simulateMessage({ type: "artifact_updated", name: "chart" });
    expect(reload).toHaveBeenCalledOnce();
  });
});

describe("residuum.watch", () => {
  it("follows team paths over the hub socket", () => {
    const { sdk, hub } = connected();
    const stopWiki = sdk.watch("team/wiki/", () => {});
    sdk.watch("team", () => {});
    stopWiki();
    expect(hub.sentFrames()).toEqual([
      { type: "watch_team", prefixes: ["team/wiki"] },
      { type: "watch_team", prefixes: ["team", "team/wiki"] },
      { type: "watch_team", prefixes: ["team"] },
    ]);
  });

  it("throws for a path outside the team, pointing at agent(name).watch", () => {
    const { sdk, hub } = connected();
    const agentPath = thrownBy(() => sdk.watch("memory/notes", () => {}));
    expect(agentPath.name).toBe("TypeError");
    expect(agentPath.message).toContain('residuum.agent("<name>").watch("memory/notes", handler)');
    const everything = thrownBy(() => sdk.watch("", () => {}));
    expect(everything.message).toContain('residuum.agent("<name>").watch("", handler)');
    expect(thrownBy(() => sdk.watch("team/../x", () => {})).message).toContain(
      "leaves the workspace",
    );
    expect(thrownBy(() => sdk.watch("/etc", () => {})).message).toContain(
      "relative to the workspace",
    );
    expect(hub.sentFrames()).toEqual([]);
  });

  it("gives each handler only the changes under its prefix, and every resync", () => {
    const { sdk, hub } = connected();
    const wiki: Frame[] = [];
    const notes: Frame[] = [];
    sdk.watch("team/wiki", (frame) => wiki.push(frame));
    sdk.watch("team/notes", (frame) => notes.push(frame));
    hub.simulateMessage(changed("team/wiki/a.md", "team/wikipedia/b.md", "team/wiki/c.md"));
    expect(wiki).toEqual([changed("team/wiki/a.md", "team/wiki/c.md")]);
    expect(notes).toEqual([]);

    hub.simulateMessage({ type: "workspace_resync", reason: "overflow" });
    expect(notes).toEqual([{ type: "workspace_resync", reason: "overflow" }]);
  });

  it("watches again after a reconnect and says changes were missed", async () => {
    vi.useFakeTimers();
    const { sdk, hub, socket } = connected();
    const seen: Frame[] = [];
    sdk.watch("team/wiki", (frame) => seen.push(frame));
    hub.simulateClose();
    await vi.advanceTimersByTimeAsync(1000);
    const again = socket(HUB);
    again.simulateOpen();
    expect(again.sentFrames()).toEqual([{ type: "watch_team", prefixes: ["team/wiki"] }]);
    expect(seen).toEqual([{ type: "workspace_resync", reason: "reconnected" }]);
  });
});

describe("residuum.agent", () => {
  const ATLAS = "/api/agents/atlas/ws";

  it("needs a name and gives one handle per agent", () => {
    const { sdk } = loadSdk();
    expect(thrownBy(() => sdk.agent("")).name).toBe("TypeError");
    expect(thrownBy(() => sdk.agent(undefined)).message).toContain('residuum.agent("scout")');
    expect(sdk.agent("atlas")).toBe(sdk.agent("atlas"));
    expect(sdk.agent("atlas").name).toBe("atlas");
  });

  it("opens the agent's socket on first use, verbose, and passes on its frames", () => {
    const { sdk, socketCount, socket } = loadSdk();
    const handle = sdk.agent("atlas");
    expect(socketCount(ATLAS)).toBe(0);

    const turns: Frame[] = [];
    const all: Frame[] = [];
    handle.on("turn_started", (frame) => turns.push(frame));
    handle.on("*", (frame) => all.push(frame));
    const atlas = socket(ATLAS);
    expect(atlas.url).toBe("ws://localhost:7702/api/agents/atlas/ws");
    atlas.simulateOpen();
    expect(atlas.sentFrames()).toEqual([{ type: "set_verbose", enabled: true }]);

    atlas.simulateMessage({ type: "turn_started", reply_to: "m1" });
    atlas.simulateMessage({ type: "tool_call", id: "t1", name: "read_file", arguments: {} });
    atlas.simulateMessage({ type: "pong" });
    atlas.simulateMessage(changed("notes/a.md"));
    expect(turns).toEqual([{ type: "turn_started", reply_to: "m1" }]);
    expect(all.map((f) => f.type)).toEqual(["connection", "turn_started", "tool_call"]);
    expect(socketCount(ATLAS)).toBe(1);
  });

  it("escapes the agent's name in the socket path", () => {
    const { sdk, socket } = loadSdk();
    sdk.agent("a/b").on("*", () => {});
    expect(socket("/api/agents/a%2Fb/ws").url).toContain("/api/agents/a%2Fb/ws");
  });

  it("watches the agent's workspace, its whole tree with ''", () => {
    const { sdk, socket } = loadSdk();
    const atlas = sdk.agent("atlas");
    const notes: Frame[] = [];
    const everything: Frame[] = [];
    atlas.watch("notes", (frame) => notes.push(frame));
    atlas.watch("", (frame) => everything.push(frame));
    const ws = socket(ATLAS);
    ws.simulateOpen();
    expect(ws.sentFrames()).toEqual([
      { type: "set_verbose", enabled: true },
      { type: "watch_workspace", prefixes: ["", "notes"] },
    ]);
    ws.simulateMessage(changed("memory/x.md", "notes/a.md"));
    expect(notes).toEqual([changed("notes/a.md")]);
    expect(everything).toEqual([changed("memory/x.md", "notes/a.md")]);
  });

  it("follows its own socket's connection, and resyncs its watches after a reconnect", async () => {
    vi.useFakeTimers();
    const { sdk, socket, hub } = connected();
    const atlas = sdk.agent("atlas");
    const states: Frame[] = [];
    const watched: Frame[] = [];
    atlas.on("connection", (frame) => states.push(frame));
    atlas.watch("notes", (frame) => watched.push(frame));
    socket(ATLAS).simulateOpen();
    socket(ATLAS).simulateClose();
    await vi.advanceTimersByTimeAsync(1000);
    socket(ATLAS).simulateOpen();
    expect(states.map((f) => f.state)).toEqual(["connected", "disconnected", "connected"]);
    expect(watched).toEqual([{ type: "workspace_resync", reason: "reconnected" }]);
    expect(socket(ATLAS).sentFrames()).toContainEqual({
      type: "watch_workspace",
      prefixes: ["notes"],
    });
    expect(hub.sentFrames()).toEqual([]);
  });
});

/** Acknowledge the page's artifact-session subscription on `hub`. */
function acknowledge(hub: FakeWebSocket, artifact = "chart"): void {
  hub.simulateMessage({ type: "subscribed", kind: "artifact_sessions", artifact });
}

/** How many of each hub socket's subscriptions the test has acknowledged. */
const acknowledged = new WeakMap<FakeWebSocket, number>();

/**
 * Start a session on `agent` that Residuum gives `address`, acknowledging the
 * page's subscription when it sent one, as the hub does.
 */
async function startSession(
  page: LoadedSdk & { hub: FakeWebSocket },
  agent: string,
  address: string,
): Promise<SessionHandle> {
  const started = page.sdk.sessions.start({ agent, prompt: "go" });
  await settle();
  const subscribes = page.hub
    .sentFrames()
    .filter((f) => (f as Frame).type === "subscribe_artifact_sessions").length;
  if (subscribes > (acknowledged.get(page.hub) ?? 0)) {
    acknowledged.set(page.hub, subscribes);
    acknowledge(page.hub);
    await settle();
  }
  lastRequest(page.requests).respond(202, { address });
  return started;
}

const relayed = (agent: string, frame: Frame): Frame => ({ type: "session_frame", agent, frame });

describe("residuum.sessions.start", () => {
  it("subscribes to the artifact's sessions and starts only once that is acknowledged", async () => {
    const { sdk, hub, requests } = connected("chart");
    const started = sdk.sessions.start({ agent: "scout", prompt: "write a page", model: "small" });
    await settle();
    expect(hub.sentFrames()).toEqual([{ type: "subscribe_artifact_sessions", artifact: "chart" }]);
    expect(requests).toEqual([]);

    acknowledge(hub);
    await settle();
    const request = lastRequest(requests);
    expect(request.method).toBe("POST");
    expect(request.url).toBe("/api/agents/scout/sessions");
    expect(request.headers.get("X-Residuum-Artifact")).toBe("chart");
    expect(JSON.parse(request.body as string)).toEqual({ prompt: "write a page", model: "small" });
    request.respond(202, { address: "artifact-chart-0001" });
    const handle = await started;
    expect(handle).toMatchObject({ agent: "scout", address: "artifact-chart-0001" });

    // Subscribed once for the page.
    void sdk.sessions.start({ agent: "atlas", prompt: "again" });
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/atlas/sessions");
    expect(hub.sentFrames()).toHaveLength(1);
  });

  it("subscribes once the hub socket opens when the page starts before that", async () => {
    const { sdk, socket } = loadSdk();
    void sdk.sessions.start({ agent: "scout", prompt: "go" });
    await settle();
    socket(HUB).simulateOpen();
    expect(socket(HUB).sentFrames()).toEqual([
      { type: "subscribe_artifact_sessions", artifact: "chart" },
    ]);
  });

  it("waits for the answer to its own subscription, not another one", async () => {
    const { sdk, hub, requests } = connected("chart");
    acknowledge(hub);
    acknowledge(hub, "other");
    void sdk.sessions.start({ agent: "scout", prompt: "go" });
    await settle();
    expect(hub.sentFrames()).toEqual([{ type: "subscribe_artifact_sessions", artifact: "chart" }]);
    acknowledge(hub, "other");
    await settle();
    expect(requests).toEqual([]);
    acknowledge(hub);
    await settle();
    expect(requests).toHaveLength(1);
  });

  it("rejects after 10 seconds without a live connection, and starts nothing", async () => {
    vi.useFakeTimers();
    const { sdk, socket, requests } = loadSdk();
    socket(HUB).simulateClose();
    const started = sdk.sessions.start({ agent: "scout", prompt: "go" }).catch((err: unknown) => ({
      ...described(err),
      code: (err as { code?: string }).code,
    }));
    await vi.advanceTimersByTimeAsync(9999);
    await settle();
    expect(requests).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(await started).toEqual({
      name: "Error",
      message: expect.stringContaining("live connection isn't available") as unknown,
      code: "no_live_connection",
    });
    expect(requests).toEqual([]);
  });

  it("needs an agent and a prompt, and sends nothing without them", async () => {
    const { sdk, hub, requests } = connected();
    for (const options of [
      { prompt: "go" },
      { agent: "", prompt: "go" },
      { agent: "scout", prompt: " " },
    ]) {
      await expect(sdk.sessions.start(options).catch(described)).resolves.toMatchObject({
        name: "TypeError",
      });
    }
    expect(requests).toEqual([]);
    expect(hub.sentFrames()).toEqual([]);
  });

  it("rejects with the agent's state when it isn't running", async () => {
    const { sdk, hub, requests } = connected();
    const started = sdk.sessions.start({ agent: "quiet", prompt: "go" }).catch((err: unknown) => ({
      ...described(err),
      ...(err as { state?: string; status?: number }),
    }));
    await settle();
    acknowledge(hub);
    await settle();
    lastRequest(requests).respond(409, { error: "quiet is stopped", state: "stopped" });
    expect(await started).toMatchObject({
      message: "quiet is stopped",
      state: "stopped",
      status: 409,
    });
  });

  it("hands a handle its session's frames from the first one, on any agent", async () => {
    const { sdk, hub, requests } = connected();
    const started = sdk.sessions.start({ agent: "atlas", prompt: "go" });
    await settle();
    acknowledge(hub);
    await settle();
    // The relay can announce the run before the start request answers.
    hub.simulateMessage(
      relayed("atlas", {
        type: "session_started",
        session: { address: "artifact-chart-1", run_id: "r1" },
      }),
    );
    hub.simulateMessage(
      relayed("atlas", {
        type: "session_started",
        session: { address: "artifact-chart-2", run_id: "r2" },
      }),
    );
    lastRequest(requests).respond(202, { address: "artifact-chart-1" });
    const handle = await started;

    const all: Frame[] = [];
    const started1: Frame[] = [];
    handle.on("*", (frame) => all.push(frame));
    handle.on("session_started", (frame) => started1.push(frame));
    await settle();
    expect(started1).toHaveLength(1);

    hub.simulateMessage(
      relayed("atlas", {
        type: "session_response",
        address: "artifact-chart-2",
        content: "theirs",
      }),
    );
    hub.simulateMessage(
      relayed("scout", {
        type: "session_response",
        address: "artifact-chart-1",
        content: "scout's",
      }),
    );
    hub.simulateMessage(
      relayed("atlas", {
        type: "session_tool_call",
        address: "artifact-chart-1",
        name: "read_file",
      }),
    );
    hub.simulateMessage(
      relayed("atlas", { type: "session_response", address: "artifact-chart-1", content: "mine" }),
    );
    expect(all.map((f) => f.type)).toEqual([
      "session_started",
      "session_tool_call",
      "session_response",
    ]);
    expect(all.at(-1)).toMatchObject({ content: "mine" });
  });

  it("messages and stops the session through its own routes", async () => {
    const page = connected();
    const { requests } = page;
    const handle = await startSession(page, "scout", "artifact-chart-1");

    const sent = handle.send("keep going");
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/scout/sessions/artifact-chart-1/messages");
    expect(JSON.parse(lastRequest(requests).body as string)).toEqual({ content: "keep going" });
    lastRequest(requests).respond(200, { outcome: "live" });
    expect(await sent).toBe("live");

    const stopped = handle.stop().catch((err: unknown) => err as { code?: string });
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/scout/sessions/artifact-chart-1/stop");
    lastRequest(requests).respond(404, { error: "not running", code: "not_live" });
    expect(await stopped).toMatchObject({ code: "not_live" });
  });

  it("tells each handle to resync with its session as listed now after relay lag", async () => {
    const page = connected();
    const { hub, requests } = page;
    const handles: SessionHandle[] = [];
    for (const [agent, address] of [
      ["atlas", "artifact-chart-1"],
      ["atlas", "artifact-chart-2"],
      ["scout", "artifact-chart-3"],
    ] as const) {
      handles.push(await startSession(page, agent, address));
    }
    const resyncs: Frame[][] = handles.map(() => []);
    handles.forEach((handle, i) => handle.on("resync", (frame) => resyncs[i]?.push(frame)));

    hub.simulateMessage({ type: "session_relay_lagged" });
    await settle();
    const reads = requests.slice(3);
    expect(reads.map((r) => r.url).sort()).toEqual([
      "/api/agents/atlas/sessions?artifact=chart",
      "/api/agents/scout/sessions?artifact=chart",
    ]);
    reads
      .find((r) => r.url.includes("atlas"))
      ?.respond(200, {
        live: [{ address: "artifact-chart-1", state: "running" }],
        completed: [{ address: "artifact-chart-2", state: "completed" }],
        next_cursor: null,
      });
    reads.find((r) => r.url.includes("scout"))?.respond(500, { error: "couldn't read sessions" });
    await settle();
    expect(resyncs).toEqual([
      [{ type: "resync", session: { address: "artifact-chart-1", state: "running" } }],
      [{ type: "resync", session: { address: "artifact-chart-2", state: "completed" } }],
      [{ type: "resync", session: null, error: "couldn't read sessions" }],
    ]);
  });

  it("subscribes again after a reconnect and resyncs its handles", async () => {
    vi.useFakeTimers();
    const page = connected();
    const { hub, requests, socket } = page;
    const handle = await startSession(page, "atlas", "artifact-chart-1");
    const seen: Frame[] = [];
    handle.on("resync", (frame) => seen.push(frame));

    hub.simulateClose();
    await vi.advanceTimersByTimeAsync(1000);
    const again = socket(HUB);
    again.simulateOpen();
    expect(again.sentFrames()).toEqual([
      { type: "subscribe_artifact_sessions", artifact: "chart" },
    ]);
    acknowledge(again);
    await settle();
    lastRequest(requests).respond(200, { live: [], completed: [], next_cursor: null });
    await settle();
    expect(seen).toEqual([{ type: "resync", session: null }]);
  });
});

describe("residuum.sessions.follow", () => {
  it("needs an agent and an address, and sends nothing without them", () => {
    const { sdk, hub, requests } = connected();
    expect(thrownBy(() => sdk.sessions.follow("", "artifact-chart-1")).name).toBe("TypeError");
    expect(thrownBy(() => sdk.sessions.follow("atlas", "")).name).toBe("TypeError");
    expect(thrownBy(() => sdk.sessions.follow(undefined, "artifact-chart-1")).message).toContain(
      "residuum.sessions.follow(agent, address)",
    );
    expect(hub.sentFrames()).toEqual([]);
    expect(requests).toEqual([]);
  });

  it("subscribes to the session directly, resyncs it with its current state, and routes its frames", async () => {
    const { sdk, hub, requests } = connected("chart");
    const handle = sdk.sessions.follow("atlas", "artifact-chart-1");
    expect(handle).toMatchObject({ agent: "atlas", address: "artifact-chart-1" });
    // Not the artifact-wide relay a start uses: the session relay's own subscription.
    expect(hub.sentFrames()).toEqual([
      { type: "subscribe_session", agent: "atlas", address: "artifact-chart-1" },
    ]);

    const all: Frame[] = [];
    handle.on("*", (frame) => all.push(frame));
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/atlas/sessions?artifact=chart");
    lastRequest(requests).respond(200, {
      live: [{ address: "artifact-chart-1", state: "running" }],
      completed: [],
      next_cursor: null,
    });
    await settle();
    expect(all).toEqual([
      { type: "resync", session: { address: "artifact-chart-1", state: "running" } },
    ]);

    hub.simulateMessage(
      relayed("atlas", { type: "session_response", address: "artifact-chart-1", content: "mine" }),
    );
    hub.simulateMessage(
      relayed("scout", {
        type: "session_response",
        address: "artifact-chart-1",
        content: "theirs",
      }),
    );
    hub.simulateMessage(
      relayed("atlas", { type: "session_response", address: "other", content: "not mine" }),
    );
    expect(all.map((f) => f.type)).toEqual(["resync", "session_response"]);
    expect(all.at(-1)).toMatchObject({ content: "mine" });
  });

  it("subscribes once the hub socket opens when the page follows before that", async () => {
    const { sdk, socket } = loadSdk();
    sdk.sessions.follow("atlas", "artifact-chart-1");
    await settle();
    socket(HUB).simulateOpen();
    expect(socket(HUB).sentFrames()).toEqual([
      { type: "subscribe_session", agent: "atlas", address: "artifact-chart-1" },
    ]);
  });

  it("messages and stops the followed session through its own routes, like a started one", async () => {
    const { sdk, requests } = connected();
    const handle = sdk.sessions.follow("scout", "artifact-chart-1");
    await settle();
    lastRequest(requests).respond(200, { live: [], completed: [], next_cursor: null });

    const sent = handle.send("keep going");
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/scout/sessions/artifact-chart-1/messages");
    lastRequest(requests).respond(200, { outcome: "live" });
    expect(await sent).toBe("live");

    const stopped = handle.stop();
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/scout/sessions/artifact-chart-1/stop");
    lastRequest(requests).respond(200, {});
    await stopped;
  });

  it("resyncs again after a reconnect, but not right after the first subscribe is acknowledged", async () => {
    vi.useFakeTimers();
    const { sdk, hub, requests, socket } = connected();
    const handle = sdk.sessions.follow("atlas", "artifact-chart-1");
    const seen: Frame[] = [];
    handle.on("resync", (frame) => seen.push(frame));
    await settle();
    lastRequest(requests).respond(200, { live: [], completed: [], next_cursor: null });
    await settle();
    expect(requests).toHaveLength(1);

    hub.simulateMessage({
      type: "subscribed",
      kind: "session",
      agent: "atlas",
      address: "artifact-chart-1",
    });
    await settle();
    expect(requests).toHaveLength(1);

    hub.simulateClose();
    await vi.advanceTimersByTimeAsync(1000);
    const again = socket(HUB);
    again.simulateOpen();
    expect(again.sentFrames()).toEqual([
      { type: "subscribe_session", agent: "atlas", address: "artifact-chart-1" },
    ]);
    again.simulateMessage({
      type: "subscribed",
      kind: "session",
      agent: "atlas",
      address: "artifact-chart-1",
    });
    await settle();
    expect(requests).toHaveLength(2);
    lastRequest(requests).respond(200, {
      live: [{ address: "artifact-chart-1", state: "idle" }],
      completed: [],
      next_cursor: null,
    });
    await settle();
    expect(seen).toEqual([
      { type: "resync", session: null },
      { type: "resync", session: { address: "artifact-chart-1", state: "idle" } },
    ]);
  });
});
