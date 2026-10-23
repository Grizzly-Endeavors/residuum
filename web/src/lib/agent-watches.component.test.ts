import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settle } from "../test/component";
import { FakeWebSocket } from "../test/fake-websocket";
import { snapshot } from "../test/hub-frames";
import { watchAgentWorkspace } from "./agent-watches.svelte";
import { hub } from "./hub.svelte";
import type { AgentState, AgentSummary } from "./hub-types";
import type { WorkspaceChange } from "./types";
import type { WatchHandler, WatchOwner } from "./watch-registry";
import { setViewedAgent } from "./viewed-agent";

// Effects decide when an agent's own connection opens and closes, so these run
// with the component tests.

function agent(name: string, state: AgentState): AgentSummary {
  return {
    name,
    display_name: name,
    state,
    last_error: null,
    autostart: false,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

function socketsOf(name: string): FakeWebSocket[] {
  return FakeWebSocket.sockets.filter((s) => new URL(s.url).pathname === `/api/agents/${name}/ws`);
}

/** The `watch_only` parameter a socket was opened with. */
function watchOnlyOf(socket: FakeWebSocket): string | null {
  return new URL(socket.url).searchParams.get("watch_only");
}

function watchFrames(socket: FakeWebSocket): unknown[] {
  return socket.sentFrames().filter((f) => (f as { type: string }).type === "watch_workspace");
}

function modified(path: string): WorkspaceChange {
  return { path, kind: "modified" };
}

interface Heard {
  changes: string[];
  resyncs: string[];
  reconnects: number;
  unavailable: string[];
}

/** A handler that writes what it hears into `heard`. */
function listener(): { handler: WatchHandler; heard: Heard } {
  const heard: Heard = { changes: [], resyncs: [], reconnects: 0, unavailable: [] };
  const handler: WatchHandler = {
    changed: (changes) => heard.changes.push(...changes.map((c) => c.path)),
    resync: (reason) => heard.resyncs.push(reason),
    reconnected: () => {
      heard.reconnects++;
    },
    unavailable: (message) => heard.unavailable.push(message),
  };
  return { handler, heard };
}

let owners: WatchOwner[] = [];

function follow(name: string, handler: WatchHandler = listener().handler): WatchOwner {
  const owner = watchAgentWorkspace(name, handler);
  owners.push(owner);
  return owner;
}

beforeEach(() => {
  FakeWebSocket.install();
  // Connecting an agent loads its history and usage; none of it matters here, so it never answers.
  vi.stubGlobal(
    "fetch",
    vi.fn(() => new Promise<Response>(() => {})),
  );
  hub.handleFrame(
    snapshot([agent("atlas", "running"), agent("scout", "running"), agent("drifter", "stopped")]),
  );
});

afterEach(() => {
  for (const owner of owners) owner.release();
  owners = [];
  setViewedAgent(null);
  vi.useRealTimers();
});

describe("following an agent's workspace", () => {
  it("goes through the agent socket for the bound agent, and opens no connection of its own", async () => {
    setViewedAgent("atlas");
    const socket = FakeWebSocket.last;
    socket.simulateOpen();
    const { handler, heard } = listener();
    follow("atlas", handler).set(["notes/a.md"]);
    await settle();

    expect(socketsOf("atlas")).toHaveLength(1);
    expect(watchFrames(socket)).toEqual([{ type: "watch_workspace", prefixes: ["notes/a.md"] }]);
    socket.simulateMessage({ type: "workspace_changed", changes: [modified("notes/a.md")] });
    expect(heard.changes).toEqual(["notes/a.md"]);
  });

  it("opens a connection to another running agent, watches over it and hears its changes", async () => {
    setViewedAgent("atlas");
    FakeWebSocket.last.simulateOpen();
    const { handler, heard } = listener();
    follow("scout", handler).set(["notes/a.md"]);
    await settle();

    const [own] = socketsOf("scout");
    expect(socketsOf("scout")).toHaveLength(1);
    expect(own).toBeDefined();
    if (own === undefined) return;
    expect(watchFrames(own)).toEqual([]);
    own.simulateOpen();
    expect(watchFrames(own)).toEqual([{ type: "watch_workspace", prefixes: ["notes/a.md"] }]);

    own.simulateMessage({
      type: "workspace_changed",
      changes: [modified("notes/a.md"), modified("notes/other.md")],
    });
    expect(heard.changes).toEqual(["notes/a.md"]);
    own.simulateMessage({ type: "workspace_resync", reason: "lagged" });
    expect(heard.resyncs).toEqual(["lagged"]);
    own.simulateMessage({ type: "workspace_watch_unavailable", message: "Live updates are off." });
    expect(heard.unavailable).toEqual(["Live updates are off."]);
  });

  it("asks the agent not to count its own connection as an open client, which the bound socket still is", async () => {
    setViewedAgent("atlas");
    const bound = FakeWebSocket.last;
    bound.simulateOpen();
    follow("scout").set(["notes/a.md"]);
    await settle();

    const [own] = socketsOf("scout");
    expect(own).toBeDefined();
    if (own === undefined) return;
    expect(watchOnlyOf(own)).toBe("true");
    expect(watchOnlyOf(bound)).toBeNull();
  });

  it("keeps the bound agent's socket free of the other agent's watch", async () => {
    setViewedAgent("atlas");
    const bound = FakeWebSocket.last;
    bound.simulateOpen();
    follow("scout").set(["notes/a.md"]);
    await settle();

    expect(watchFrames(bound)).toEqual([]);
  });

  it("tells its handler to read again after the connection drops and returns", async () => {
    vi.useFakeTimers();
    setViewedAgent("atlas");
    const { handler, heard } = listener();
    follow("scout", handler).set(["notes/a.md"]);
    await settle();
    const first = socketsOf("scout")[0];
    if (first === undefined) throw new Error("no connection to scout");
    first.simulateOpen();
    expect(heard.reconnects).toBe(0);

    first.simulateClose();
    await vi.advanceTimersByTimeAsync(2000);
    const second = socketsOf("scout")[1];
    if (second === undefined) throw new Error("no second connection to scout");
    second.simulateOpen();

    expect(heard.reconnects).toBe(1);
    expect(watchFrames(second)).toEqual([{ type: "watch_workspace", prefixes: ["notes/a.md"] }]);
  });

  it("shares one connection between owners, and closes it when the last lets go", async () => {
    setViewedAgent("atlas");
    const first = follow("scout");
    const second = follow("scout");
    first.set(["a.md"]);
    second.set(["b.md"]);
    await settle();
    const own = socketsOf("scout");
    expect(own).toHaveLength(1);
    own[0]?.simulateOpen();
    expect(watchFrames(own[0] as FakeWebSocket).at(-1)).toEqual({
      type: "watch_workspace",
      prefixes: ["a.md", "b.md"],
    });

    first.release();
    await settle();
    expect(own[0]?.readyState).toBe(FakeWebSocket.OPEN);
    second.release();
    await settle();
    expect(own[0]?.readyState).toBe(FakeWebSocket.CLOSED);

    follow("scout").set(["a.md"]);
    await settle();
    expect(socketsOf("scout")).toHaveLength(2);
  });

  it("opens no connection to a stopped agent, and opens one when the agent runs", async () => {
    setViewedAgent("atlas");
    const { handler, heard } = listener();
    follow("drifter", handler).set(["notes/a.md"]);
    await settle();
    expect(socketsOf("drifter")).toHaveLength(0);

    hub.handleFrame({ type: "agent_state", agent: agent("drifter", "starting") });
    await settle();
    expect(socketsOf("drifter")).toHaveLength(0);

    hub.handleFrame({ type: "agent_state", agent: agent("drifter", "running") });
    await settle();
    const [own] = socketsOf("drifter");
    if (own === undefined) throw new Error("no connection to drifter");
    own.simulateOpen();
    expect(watchFrames(own)).toEqual([{ type: "watch_workspace", prefixes: ["notes/a.md"] }]);
    own.simulateMessage({ type: "workspace_changed", changes: [modified("notes/a.md")] });
    expect(heard.changes).toEqual(["notes/a.md"]);
  });

  it("closes the connection when the agent stops, and doesn't try again", async () => {
    vi.useFakeTimers();
    setViewedAgent("atlas");
    follow("scout").set(["notes/a.md"]);
    await settle();
    const own = socketsOf("scout")[0];
    if (own === undefined) throw new Error("no connection to scout");
    own.simulateOpen();

    // The agent's shutdown closes the socket before the hub reports it stopped.
    own.simulateClose();
    hub.handleFrame({ type: "agent_state", agent: agent("scout", "stopped") });
    await settle();
    await vi.advanceTimersByTimeAsync(60_000);

    expect(socketsOf("scout")).toHaveLength(1);
  });

  it("opens none for an agent the hub doesn't list", async () => {
    setViewedAgent("atlas");
    follow("ghost").set(["notes/a.md"]);
    await settle();
    expect(socketsOf("ghost")).toHaveLength(0);
  });

  it("hands over to the agent socket when the agent becomes the bound one, and back", async () => {
    setViewedAgent("atlas");
    const { handler, heard } = listener();
    follow("scout", handler).set(["notes/a.md"]);
    await settle();
    const own = socketsOf("scout")[0];
    if (own === undefined) throw new Error("no connection to scout");
    own.simulateOpen();

    setViewedAgent("scout");
    await settle();
    expect(own.readyState).toBe(FakeWebSocket.CLOSED);
    const bound = socketsOf("scout")[1];
    if (bound === undefined) throw new Error("the agent socket did not open for scout");
    bound.simulateOpen();
    expect(watchFrames(bound)).toEqual([{ type: "watch_workspace", prefixes: ["notes/a.md"] }]);
    bound.simulateMessage({ type: "workspace_changed", changes: [modified("notes/a.md")] });
    expect(heard.changes).toEqual(["notes/a.md"]);

    setViewedAgent("atlas");
    await settle();
    expect(bound.readyState).toBe(FakeWebSocket.CLOSED);
    expect(socketsOf("scout")).toHaveLength(3);
  });

  it("refuses a prefix the gateway would refuse, as the socket's registry does", () => {
    setViewedAgent("atlas");
    const owner = follow("scout");
    expect(() => {
      owner.set(["../secret"]);
    }).toThrow(TypeError);
  });
});
