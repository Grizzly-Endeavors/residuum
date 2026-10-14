import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { jsonResponse, mockFetch, settle } from "../test/component";
import { FakeWebSocket } from "../test/fake-websocket";
import { snapshot } from "../test/hub-frames";
import { hub } from "./hub.svelte";
import type { AgentState, AgentSummary } from "./hub-types";
import { runningCount } from "./running-count";
import type { OutboundA2aTaskSummary } from "./generated/protocol";
import type { SessionSummary } from "./types";
import { setViewedAgent } from "./viewed-agent";
import { ws } from "./ws.svelte";

// The number the rail's Activity row and the Chat's header pill both show.
// The bound agent follows the viewed one through effects, so this lives with
// the component tests.

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

function session(runId: string): SessionSummary {
  return {
    address: `spawned-${runId}`,
    run_id: runId,
    category: "spawned",
    source_label: "agent:researcher",
    state: "running",
    spawner: null,
    depth: 1,
    purpose: "",
    started_at: "2026-09-23T12:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
  };
}

function task(taskId: string): OutboundA2aTaskSummary {
  return {
    task_id: taskId,
    agent: "remote",
    sender_address: "atlas",
    state: "working",
    status_text: null,
    open: true,
    started_at: "2026-09-23T12:00:00Z",
    unreachable_since: null,
  };
}

beforeEach(() => {
  FakeWebSocket.install();
  mockFetch((url) => {
    if (url.includes("/sessions")) {
      return jsonResponse({ live: [], completed: [], next_cursor: null });
    }
    if (url.includes("/chat/history")) {
      return jsonResponse({ kind: "recent", messages: [], next_cursor: null });
    }
    if (url.includes("/usage")) return jsonResponse({ input_tokens: 0, output_tokens: 0 });
    return jsonResponse([]);
  });
  hub.handleFrame(snapshot([agent("atlas", "running"), agent("scout", "running")]));
});

afterEach(() => {
  setViewedAgent(null);
});

async function bindAtlasWithRunsOut(): Promise<void> {
  setViewedAgent("atlas");
  await settle();
  ws.sessions.handleFrame({ type: "session_started", session: session("r1") });
  ws.sessions.handleFrame({ type: "session_started", session: session("r2") });
  ws.sessions.applyOutbound(task("t1"));
}

describe("runningCount", () => {
  it("counts live sessions and open outbound tasks together", async () => {
    await bindAtlasWithRunsOut();
    expect(runningCount("atlas")).toBe(3);
    expect(runningCount("atlas")).toBe(ws.sessions.runningCount);
  });

  it("is zero for an agent the page hasn't bound", async () => {
    await bindAtlasWithRunsOut();
    expect(runningCount("scout")).toBe(0);
  });

  it("is zero once the agent is no longer up, and counts while it is stopping", async () => {
    await bindAtlasWithRunsOut();

    hub.handleFrame({ type: "agent_stopping", name: "atlas" });
    expect(runningCount("atlas")).toBe(3);

    hub.handleFrame({ type: "agent_state", agent: agent("atlas", "stopped") });
    expect(runningCount("atlas")).toBe(0);
  });
});
