import { randomUUID } from "node:crypto";
import type { AgentActivity, AgentListResponse, AgentSummary } from "../src/lib/generated/protocol";
import type { HubServerMessage } from "../src/lib/hub-types";
import { openAgentSocket } from "./agent-socket";
import { createHubConfigReloader } from "./hub-config-reload";
import { openHubSocket } from "./hub-socket";
import type { UpgradeHost } from "./sockets";
import { createState, seedAgentData, type MockAgent, type MockHub } from "./state";
import { byName } from "./util";

export function mockAgentSummary(agent: MockAgent): AgentSummary {
  return {
    name: agent.name,
    state: agent.runState,
    last_error: agent.runState === "failed" ? agent.lastError : null,
    autostart: agent.autostart,
    role: agent.role,
    a2a_visibility: agent.visibility,
  };
}

/** An agent's main-conversation activity. */
export function mockActivity(agent: MockAgent): AgentActivity {
  return { busy: agent.busySince !== null, busy_since: agent.busySince, unread: agent.unread };
}

/** `agent_activity`, the frame that carries an agent's activity after it changes. */
function activityFrame(agent: MockAgent): HubServerMessage {
  return { type: "agent_activity", name: agent.name, ...mockActivity(agent) };
}

/**
 * The agents by name with the activity of each and the names whose stop has
 * begun, like the backend's `AgentListResponse`.
 */
export function mockListing(agents: Iterable<MockAgent>): AgentListResponse {
  const sorted = [...agents].sort((a, b) => byName(a.name, b.name));
  return {
    agents: sorted.map(mockAgentSummary),
    activity: Object.fromEntries(sorted.map((agent) => [agent.name, mockActivity(agent)])),
    stopping: sorted.filter((agent) => agent.stopping).map((agent) => agent.name),
  };
}

/**
 * An agent's conversation begins when it first runs. The agent opens with a
 * greeting, except the first, so its chat is told apart from that one's.
 */
function startConversation(agent: MockAgent): void {
  seedAgentData(agent.state);
  if (agent.name !== "scout") {
    agent.state.extraRecent.push({
      role: "assistant",
      content: `Hi, this is ${agent.name}. You are in my conversation, not scout's.`,
      timestamp: new Date().toISOString(),
      visibility: "user",
    });
  }
}

/**
 * The hub: its agents, the hub WebSocket, and the agent WebSockets it opens on
 * the HTTP server. It starts with no agents.
 */
export function createHub(host: UpgradeHost | null): MockHub {
  const agents = new Map<string, MockAgent>();
  const hubState = createState("hub");

  const listing = (): AgentListResponse => mockListing(agents.values());
  const { broadcast } = openHubSocket(host, randomUUID(), listing);
  const reloadHubConfig = createHubConfigReloader(hubState, broadcast);

  const hub: MockHub = {
    agents,
    deleted: new Map(),
    hubState,
    broadcast,
    summary: mockAgentSummary,
    listing,
    reloadHubConfig,
    createAgent(name, options = {}) {
      const runState = options.runState ?? "running";
      const agent: MockAgent = {
        name,
        runState,
        lastError:
          options.lastError === undefined
            ? null
            : { ...options.lastError, at: new Date().toISOString() },
        autostart: options.runState !== "stopped",
        role: options.role ?? null,
        visibility: "private",
        busySince: null,
        stopping: false,
        unread: 0,
        state: createState(name, false),
        connectedClients: () => 0,
      };
      if (runState === "running") startConversation(agent);
      agents.set(name, agent);
      openAgentSocket(host, hub, agent);
      return agent;
    },
    setBusy(agent, busy) {
      agent.busySince = busy ? (agent.busySince ?? new Date().toISOString()) : null;
      broadcast(activityFrame(agent));
    },
    addUnread(agent) {
      agent.unread += 1;
      broadcast(activityFrame(agent));
    },
    clearUnread(agent) {
      if (agent.unread === 0) return;
      agent.unread = 0;
      broadcast(activityFrame(agent));
    },
    markStopping(agent) {
      agent.stopping = true;
      broadcast({ type: "agent_stopping", name: agent.name });
    },
    transition(agent, runState) {
      agent.runState = runState;
      agent.stopping = false;
      if (runState === "running" && !agent.state.hasConversation) startConversation(agent);
      if (runState !== "running") {
        agent.state.dropSockets();
        // An agent that is no longer running has no turn in progress.
        if (agent.busySince !== null) {
          agent.busySince = null;
          broadcast(activityFrame(agent));
        }
      }
      broadcast({ type: "agent_state", agent: mockAgentSummary(agent) });
    },
  };

  return hub;
}
