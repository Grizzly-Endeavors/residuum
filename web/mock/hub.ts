import { randomUUID } from "node:crypto";
import type { AgentActivity, AgentListResponse, AgentSummary } from "../src/lib/generated/protocol";
import type { HubServerMessage } from "../src/lib/hub-types";
import { openAgentSocket } from "./agent-socket";
import { HUB_STATE_NAME, MOCK_DETERMINISTIC_BOOT_ID } from "./constants";
import { createMockEnv, type MockEnv } from "./env";
import { createHubConfigReloader } from "./hub-config-reload";
import { openHubSocket } from "./hub-socket";
import { createOverview } from "./overview";
import { sessionAddressOf, type SessionEventFrame } from "./session-relay";
import type { UpgradeHost } from "./sockets";
import { createState, seedAgentData, type MockAgent, type MockHub, type MockState } from "./state";
import { createTeamEvents } from "./team-events";
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
      timestamp: agent.state.env.clock.iso(),
      visibility: "user",
    });
  }
}

/**
 * The source label of the session a frame is about: the one on its start
 * frame, or the one its run was registered with, which is how the backend's
 * watcher knows a later event's label.
 */
function sourceLabelOf(state: MockState, frame: SessionEventFrame): string | null {
  if (frame.type === "session_started") return frame.session.source_label;
  const address = sessionAddressOf(frame);
  const { live, completed } = state.sessions;
  const run =
    live.find((s) => s.address === address) ?? completed.find((s) => s.address === address);
  return run?.source_label ?? null;
}

export interface HubOptions {
  /** The clock, delays and timers the hub and its agents share. Live ones by default. */
  env?: MockEnv;
  /** Creates the agents the hub starts with, and again after every reset. The hub starts with none by default. */
  seed?: (hub: MockHub) => void;
}

/**
 * The hub: its agents, the hub WebSocket, and the agent WebSockets it opens on
 * the HTTP server.
 */
export function createHub(
  host: UpgradeHost | null,
  { env = createMockEnv(), seed }: HubOptions = {},
): MockHub {
  const agents = new Map<string, MockAgent>();
  const hubState = createState(HUB_STATE_NAME, true, env);

  const listing = (): AgentListResponse => mockListing(agents.values());
  const bootId = env.deterministic ? MOCK_DETERMINISTIC_BOOT_ID : randomUUID();
  const {
    broadcast: sendToPages,
    relaySession,
    lagSessionRelay,
    dropClients,
    presentDevices,
  } = openHubSocket(host, bootId, listing, env.clock, (name) => agents.has(name));
  const teamEvents = createTeamEvents(env, bootId, sendToPages);
  const overview = createOverview(env, bootId, agents, sendToPages);
  // Every frame the hub sends is also read by the log and the overview, as
  // the backend's recorder and tracker read the hub bus.
  const broadcast = (frame: HubServerMessage): void => {
    sendToPages(frame);
    teamEvents.observeHub(frame);
    overview.observeHub(frame);
  };
  /** The log of a hub that has just started the agents the scenario created. */
  const beginLog = (): void => {
    teamEvents.begin(
      [...agents.values()].sort((a, b) => byName(a.name, b.name)).map(mockAgentSummary),
    );
    overview.begin();
  };

  const hub: MockHub = {
    env,
    agents,
    deleted: new Map(),
    hubState,
    broadcast,
    relaySession: (agent, frame) => {
      relaySession(agent.name, frame, sourceLabelOf(agent.state, frame));
    },
    lagSessionRelay,
    presentPushDevices: presentDevices,
    teamEvents,
    overview,
    summary: mockAgentSummary,
    listing,
    reloadHubConfig: createHubConfigReloader(hubState, broadcast),
    createAgent(name, options = {}) {
      const runState = options.runState ?? "running";
      const agent: MockAgent = {
        name,
        runState,
        lastError:
          options.lastError === undefined ? null : { ...options.lastError, at: env.clock.iso() },
        autostart: options.runState !== "stopped",
        role: options.role ?? null,
        visibility: "private",
        busySince: null,
        stopping: false,
        unread: 0,
        state: createState(name, false, env),
        connectedClients: () => 0,
        dispose: () => undefined,
      };
      if (runState === "running") startConversation(agent);
      agents.set(name, agent);
      openAgentSocket(host, hub, agent);
      teamEvents.watchAgent(agent);
      overview.watchAgent(agent);
      return agent;
    },
    setBusy(agent, busy) {
      agent.busySince = busy ? (agent.busySince ?? env.clock.iso()) : null;
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
    reset({ setup = false } = {}) {
      env.reset();
      dropClients();
      const gone = [...hub.deleted.values()].map((deleted) => deleted.agent);
      for (const agent of [...agents.values(), ...gone]) agent.dispose();
      agents.clear();
      hub.deleted.clear();
      // The state is replaced in place: the artifacts listener holds it, and
      // the port it listens on is not part of the scenario.
      const { workbenchPort } = hubState;
      Object.assign(hubState, createState(HUB_STATE_NAME, true, env));
      hubState.workbenchPort = workbenchPort;
      hub.reloadHubConfig = createHubConfigReloader(hubState, broadcast);
      if (setup) hubState.mode = "setup";
      else seed?.(hub);
      beginLog();
    },
  };

  seed?.(hub);
  beginLog();
  return hub;
}
