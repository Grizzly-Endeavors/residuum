import type { AgentSummary } from "../src/lib/generated/protocol";
import type { HubServerMessage } from "../src/lib/hub-types";
import { openAgentSocket } from "./agent-socket";
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

  const sortedSummaries = (): AgentSummary[] =>
    [...agents.values()].sort((a, b) => byName(a.name, b.name)).map(mockAgentSummary);

  // A page that connects gets the agent list, then the activity of every
  // agent with something to show.
  const greeting = (): HubServerMessage[] => {
    const frames: HubServerMessage[] = [{ type: "agents_snapshot", agents: sortedSummaries() }];
    for (const agent of agents.values()) {
      if (agent.busy || agent.unread > 0) {
        frames.push({
          type: "agent_activity",
          name: agent.name,
          busy: agent.busy,
          unread: agent.unread,
        });
      }
    }
    return frames;
  };
  const { broadcast } = openHubSocket(host, greeting);

  const hub: MockHub = {
    agents,
    deleted: new Map(),
    hubState,
    broadcast,
    summary: mockAgentSummary,
    createAgent(name, options = {}) {
      const runState = options.runState ?? "running";
      const agent: MockAgent = {
        name,
        runState,
        lastError:
          options.lastError === undefined
            ? null
            : { message: options.lastError, at: new Date().toISOString() },
        autostart: options.runState !== "stopped",
        role: options.role ?? null,
        visibility: "private",
        busy: false,
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
      agent.busy = busy;
      broadcast({ type: "agent_activity", name: agent.name, busy, unread: agent.unread });
    },
    addUnread(agent) {
      agent.unread += 1;
      broadcast({
        type: "agent_activity",
        name: agent.name,
        busy: agent.busy,
        unread: agent.unread,
      });
    },
    clearUnread(agent) {
      if (agent.unread === 0) return;
      agent.unread = 0;
      broadcast({ type: "agent_activity", name: agent.name, busy: agent.busy, unread: 0 });
    },
    transition(agent, runState) {
      agent.runState = runState;
      if (runState === "running" && !agent.state.hasConversation) startConversation(agent);
      if (runState !== "running") agent.state.dropSockets();
      broadcast({ type: "agent_state", agent: mockAgentSummary(agent) });
    },
  };

  return hub;
}
