import type {
  A2aVisibility,
  AgentLastError,
  AgentListResponse,
  AgentState,
  AgentSummary,
  OutboundA2aTaskSummary,
  ServerMessage,
} from "../src/lib/generated/protocol";
import type { HubServerMessage } from "../src/lib/hub-types";
import type { RecentMessage, UserInboxItem, WorkspaceEntry } from "../src/lib/types";
import { loadAsset } from "./assets";
import { seedCheckpoints, type MockCheckpoints } from "./checkpoints";
import { createArchivedInboxItems, createInboxItems } from "./data/inbox";
import { createSessions, type MockSessions } from "./data/sessions";
import { createWorkspaceFileContents, createWorkspaceFiles } from "./data/workspace";
import { createMockEnv, type MockEnv } from "./env";
import type { MockPushDevice } from "./push";
import { createScheduled, type MockScheduled } from "./scheduled";
import type { MockOverview } from "./overview";
import type { SessionEventFrame } from "./session-relay";
import type { MockTeamEvents } from "./team-events";
import type { MockCloud } from "./cloud";
import type { MockUpdateStatus } from "./update";

/** An agent key as the mock stores it, value included. */
export interface MockAgentKey {
  value: string;
  description: string;
  created_by: "user" | "agent";
}

/** An A2A caller key as the mock stores it. */
export interface MockA2aKey {
  description: string;
  created_at: string;
}

/** An item in an agent's own inbox, which a workbench artifact can add to. */
export interface MockAgentInboxItem {
  id: string;
  title: string;
  body: string;
  source: string;
  timestamp: string;
}

export interface MockState {
  /** The agent this state belongs to, or `hub` for the hub- and team-level state. */
  agentName: string;
  /** The clock, delays and timers the mock shares; the hub and every agent hold the same one. */
  env: MockEnv;
  mode: "setup" | "running";
  secrets: Map<string, string>;
  agentKeys: Map<string, MockAgentKey>;
  a2aKeys: Map<string, MockA2aKey>;
  a2aAgentsJson: string;
  configToml: string;
  hubConfigToml: string;
  providersToml: string;
  /** Directory path to its listing. Changed only through `workspace-tree.ts`, which keeps it agreeing with the contents. */
  workspaceFiles: Record<string, WorkspaceEntry[]>;
  /** File path to its content. `config/mcp.json` (`MCP_JSON`) is the agent's MCP servers. */
  workspaceFileContents: Record<string, string>;
  inboxItems: UserInboxItem[];
  /** The items the user archived, which `restore` brings back to `inboxItems`. */
  inboxArchive: UserInboxItem[];
  /** What a workbench artifact added to the agent's own inbox (`POST /api/agent-inbox`), oldest first. */
  agentInbox: MockAgentInboxItem[];
  /** The pulses and scheduled actions the Scheduled view lists. */
  scheduled: MockScheduled;
  /** What the hub knows about updates, for the update routes. */
  update: MockUpdateStatus;
  /** What a test holds about the Residuum Cloud tunnel, for the cloud routes. Only the hub's state is read. */
  cloud: MockCloud;
  /** The devices registered for Web Push, oldest first. Only the hub's state holds any. */
  pushDevices: MockPushDevice[];
  /** The checkpoint histories this state holds: an agent's own, or the hub's. */
  checkpoints: MockCheckpoints;
  /**
   * Whether the agent has a conversation: the sample history and episodes
   * sit behind `extraRecent`. An agent has one once it has run.
   */
  hasConversation: boolean;
  sessions: MockSessions;
  /** Port of the mock artifacts listener, once it is listening. */
  workbenchPort: number | null;
  /** Main-agent messages recorded after the sample history (see `/api/mock/missed-relay`). */
  extraRecent: RecentMessage[];
  /** Close every WebSocket, as if the connection dropped. Set by the agent socket. */
  dropSockets: () => void;
  /**
   * Send a frame to every connected WebSocket client, the way a session
   * frame reaches the sidebar. Set by the agent socket; the REST handlers
   * for the artifact session endpoints use it to announce sessions they
   * start, stop, or message the same way the WebSocket command handlers do.
   * A change feed frame (`workspace_changed` and the like) goes only to the
   * clients whose `watch_workspace` prefixes it touches.
   */
  broadcast: (frame: ServerMessage) => void;
  /**
   * Open tasks sent to remote agents, for the sessions sidebar. `laptop`'s
   * task is unreachable, so stopping it answers `502 unreachable` and the
   * row offers "Stop watching".
   */
  outboundTasks: OutboundA2aTaskSummary[];
  /**
   * Set once a "drop compress" chat message has simulated the observer
   * compressing history into `ep-004`: how many `extraRecent` entries went
   * into that episode.
   */
  compressedAt: number | null;
}

export interface MockAgent {
  name: string;
  runState: AgentState;
  lastError: AgentLastError | null;
  autostart: boolean;
  role: string | null;
  visibility: A2aVisibility;
  /** When the current main turn began, or `null` while none is running. */
  busySince: string | null;
  /** The agent's stop has begun and isn't finished: its state is still `running`. */
  stopping: boolean;
  /** Main-conversation messages the web UI hasn't shown. */
  unread: number;
  state: MockState;
  /** How many web clients have this agent's WebSocket open. Set by the agent socket. */
  connectedClients: () => number;
  /** Close the agent's WebSocket route and its connections. Set by the agent socket. */
  dispose: () => void;
}

/** An agent removed by `DELETE`, kept whole so a restore brings back its conversation and settings. */
export interface MockDeletedAgent {
  agent: MockAgent;
  deletedAt: string;
  checkpointId: string;
}

export interface MockHub {
  env: MockEnv;
  agents: Map<string, MockAgent>;
  /** Deleted agents that can be restored, by name. */
  deleted: Map<string, MockDeletedAgent>;
  /** Hub-level and team-level state: secrets, hub config, team files, the workbench. */
  hubState: MockState;
  /** Register an agent and open its WebSocket route. */
  createAgent: (
    name: string,
    options?: {
      role?: string | null;
      runState?: AgentState;
      lastError?: Omit<AgentLastError, "at">;
    },
  ) => MockAgent;
  summary: (agent: MockAgent) => AgentSummary;
  /** Every agent by name with its activity and stopping set: `GET /api/hub/agents` and the hub snapshot. */
  listing: () => AgentListResponse;
  /** Send a frame to every hub WebSocket client. */
  broadcast: (frame: HubServerMessage) => void;
  /**
   * Send an event of one of an agent's sessions to the hub WebSocket clients
   * that follow it, as the hub's session relay does. The agent's socket calls
   * this for every session frame it broadcasts.
   */
  relaySession: (agent: MockAgent, frame: SessionEventFrame) => void;
  /**
   * Tell the hub WebSocket clients that follow sessions that they lost
   * frames, as a connection that fell behind the relay is told. Returns how
   * many clients were told.
   */
  lagSessionRelay: () => number;
  /**
   * The push devices whose page is connected to the hub socket and reported
   * `presence` active within the last minute: the ones the hub sends no push.
   */
  presentPushDevices: () => string[];
  /** What has happened across the team since the hub started: `GET /api/hub/events` and the `team_event` frames. */
  teamEvents: MockTeamEvents;
  /** What Home shows about each agent: `GET /api/hub/overview` and the `agent_overview` frames. */
  overview: MockOverview;
  setBusy: (agent: MockAgent, busy: boolean) => void;
  /** Tell hub clients the agent's stop has begun. Its state changes when `transition` moves it on. */
  markStopping: (agent: MockAgent) => void;
  /**
   * Reload the hub config from the state's `hubConfigToml` the way the hub
   * does after the file changes, and tell hub clients how it went.
   */
  reloadHubConfig: () => void;
  addUnread: (agent: MockAgent) => void;
  clearUnread: (agent: MockAgent) => void;
  /** Move an agent to a run state and tell hub clients. */
  transition: (agent: MockAgent, runState: AgentState) => void;
  /** Take the hub WebSocket down or bring it back (see `HubSocket.setOnline`). A reset brings it back. */
  setHubSocketOnline: (online: boolean) => void;
  /**
   * Put the mock back as it started: the clock, the timers and delays, the
   * hub's own state, and the agents the scenario creates. Every socket is
   * closed, so pages reconnect to the new state. With `setup`, it starts with
   * no agents instead, as a hub that hasn't been set up.
   */
  reset: (options?: { setup?: boolean }) => void;
}

/** The remote agents an agent that has run has listed in its A2A client settings. */
const SAMPLE_A2A_AGENTS_JSON =
  JSON.stringify(
    { agents: { "research-buddy": { url: "https://example.com/a2a/research-buddy" } } },
    null,
    2,
  ) + "\n";

/** The A2A client settings of an agent with no remote agents listed. */
const EMPTY_A2A_AGENTS_JSON = '{"agents":{}}';

/** Where an agent's MCP servers live in its workspace, which the MCP routes read and write. */
export const MCP_JSON = "config/mcp.json";

/**
 * Give an agent the data it has once it has run: a conversation, the sample
 * inbox and archive, and its A2A client settings. An agent that has never run
 * has none of it, and its file-only routes answer with empty data.
 */
export function seedAgentData(state: MockState): void {
  const { clock } = state.env;
  state.hasConversation = true;
  state.inboxItems = createInboxItems(clock);
  state.inboxArchive = createArchivedInboxItems(clock);
  state.a2aAgentsJson = SAMPLE_A2A_AGENTS_JSON;
  state.scheduled = createScheduled(clock);
}

/**
 * A fresh state with the sample data of an agent that has run, or with none
 * of it when `hasRun` is false.
 */
export function createState(
  agentName: string,
  hasRun = true,
  env: MockEnv = createMockEnv(),
): MockState {
  const { clock } = env;
  const workspaceFileContents = {
    ...createWorkspaceFileContents(),
    [MCP_JSON]: loadAsset("mcp.example.json"),
  };
  const state: MockState = {
    agentName,
    env,
    mode: process.env.VITE_MOCK_SETUP === "1" ? "setup" : "running",
    workbenchPort: null,
    secrets: new Map([
      ["anthropic_key", "sk-ant-mock-xxxx"],
      ["openai_key", "sk-mock-xxxx"],
    ]),
    agentKeys: new Map([
      [
        "github_token",
        {
          value: "ghp_mock_xxxxxxxx",
          description: "Fine-grained token, read/write on my repos",
          created_by: "user",
        },
      ],
      [
        "cf_session",
        {
          value: "cf_mock_xxxxxxxx",
          description: "Short-lived Cloudflare API token minted for DNS updates",
          created_by: "agent",
        },
      ],
    ]),
    a2aKeys: new Map([
      [
        "laptop",
        {
          description: "My other instance, before siblings exist",
          created_at: clock.isoAgo(3 * 86_400_000),
        },
      ],
    ]),
    a2aAgentsJson: EMPTY_A2A_AGENTS_JSON,
    configToml: loadAsset("config.example.toml"),
    hubConfigToml: loadAsset("hub-config.example.toml"),
    providersToml: loadAsset("providers.example.toml"),
    workspaceFiles: createWorkspaceFiles(workspaceFileContents, clock),
    workspaceFileContents,
    sessions: createSessions(clock),
    outboundTasks: [
      {
        task_id: "task-7f3a",
        agent: "research-buddy",
        sender_address: "main",
        state: "working",
        status_text: "Reading the three papers you linked and pulling out their benchmark numbers.",
        open: true,
        started_at: clock.isoAgo(4 * 60_000),
        unreachable_since: null,
      },
      {
        task_id: "task-19c2",
        agent: "laptop",
        sender_address: "main",
        state: "working",
        status_text: null,
        open: true,
        started_at: clock.isoAgo(42 * 60_000),
        unreachable_since: clock.isoAgo(17 * 60_000),
      },
    ],
    extraRecent: [],
    dropSockets: () => {},
    broadcast: () => {},
    compressedAt: null,
    inboxItems: [],
    inboxArchive: [],
    agentInbox: [],
    scheduled: { pulses: [], actions: [] },
    update: { latest: null, lastChecked: null },
    cloud: { tunnel: null, viaTunnel: false },
    pushDevices: [],
    checkpoints: {},
    hasConversation: false,
  };
  if (hasRun) seedAgentData(state);
  seedCheckpoints(state);
  return state;
}
