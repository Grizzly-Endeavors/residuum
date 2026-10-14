import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { jsonResponse, mockFetch, settle } from "../test/component";
import { FakeWebSocket } from "../test/fake-websocket";
import { snapshot } from "../test/hub-frames";
import { hub } from "./hub.svelte";
import type { AgentState, AgentSummary } from "./hub-types";
import { setViewedAgent } from "./viewed-agent";
import { ws } from "./ws.svelte";
import { waitFor } from "../test/wait";

// The agent connection follows the hub's word on the bound agent: it is open
// while the agent runs and closed otherwise, with history read from files.
// Effects run here, so these live with the component tests.

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

function agentSockets(name: string): FakeWebSocket[] {
  return FakeWebSocket.sockets.filter((s) => s.url.endsWith(`/api/agents/${name}/ws`));
}

let historyFetches = 0;

beforeEach(() => {
  FakeWebSocket.install();
  historyFetches = 0;
  mockFetch((url) => {
    if (url.includes("/chat/history")) {
      historyFetches++;
      return jsonResponse({
        kind: "recent",
        messages: [
          { role: "assistant", content: "Said before it stopped.", timestamp: "2026-03-14T11:00" },
        ],
        next_cursor: null,
      });
    }
    if (url.includes("/usage")) return jsonResponse({ input_tokens: 0, output_tokens: 0 });
    if (url.includes("/sessions"))
      return jsonResponse({ live: [], completed: [], next_cursor: null });
    return jsonResponse([]);
  });
  hub.handleFrame(snapshot([agent("atlas", "running"), agent("drifter", "stopped")]));
});

afterEach(() => {
  setViewedAgent(null);
  vi.useRealTimers();
});

describe("the agent connection", () => {
  it("stays closed for a stopped agent, whose history still loads", async () => {
    setViewedAgent("drifter");
    await waitFor(() => {
      expect(ws.store.feed.map((item) => ("content" in item ? item.content : ""))).toEqual([
        "Said before it stopped.",
      ]);
    });

    expect(agentSockets("drifter")).toHaveLength(0);
    expect(ws.transport.status).toBe("disconnected");
  });

  it("opens once the agent starts, and catches the chat up", async () => {
    setViewedAgent("drifter");
    await settle();
    expect(historyFetches).toBe(1);

    hub.handleFrame({ type: "agent_state", agent: agent("drifter", "starting") });
    await settle();
    expect(agentSockets("drifter")).toHaveLength(0);

    hub.handleFrame({ type: "agent_state", agent: agent("drifter", "running") });
    await settle();
    expect(agentSockets("drifter")).toHaveLength(1);
    FakeWebSocket.last.simulateOpen();
    await settle();
    expect(historyFetches).toBe(2);
  });

  it("closes once the agent stops, and doesn't try again", async () => {
    vi.useFakeTimers();
    setViewedAgent("atlas");
    const socket = FakeWebSocket.last;
    socket.simulateOpen();
    ws.store.pushUserMessage("Long job");
    ws.store.handleMessage({
      type: "turn_started",
      reply_to: "m1",
      origin: { endpoint: "ws", visibility: "user" },
    });

    hub.handleFrame({ type: "agent_stopping", name: "atlas" });
    await settle();
    expect(ws.transport.status).toBe("connected");

    // The agent's shutdown closes the socket before the hub reports it stopped.
    socket.simulateClose();
    hub.handleFrame({ type: "agent_state", agent: agent("atlas", "stopped") });
    await settle();
    await vi.advanceTimersByTimeAsync(60_000);

    expect(agentSockets("atlas")).toHaveLength(1);
    expect(ws.transport.status).toBe("disconnected");
    expect(ws.transport.lost).toBe(false);
    // Nothing more of the turn will come.
    expect(ws.store.isProcessing).toBe(false);
    expect(ws.store.feed.at(-1)).toMatchObject({ kind: "user", content: "Long job" });
  });
});
