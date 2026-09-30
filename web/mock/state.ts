import type {
  A2aVisibility,
  AgentLastError,
  AgentState,
  AgentSummary,
  OutboundA2aTaskSummary,
  ServerMessage,
} from "../src/lib/generated/protocol";
import type { HubServerMessage } from "../src/lib/hub-types";
import type { RecentMessage, UserInboxItem } from "../src/lib/types";
import { loadAsset } from "./assets";
import { createInboxItems } from "./data/inbox";
import { createSessions, type MockSessions } from "./data/sessions";
import { createWorkbenchArtifacts, type MockArtifact } from "./data/workbench";
import {
  createWorkspaceFileContents,
  createWorkspaceFiles,
  type MockWorkspaceEntry,
} from "./data/workspace";

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

export interface MockState {
  /** The agent this state belongs to, or `hub` for the hub- and team-level state. */
  agentName: string;
  mode: "setup" | "running";
  secrets: Map<string, string>;
  agentKeys: Map<string, MockAgentKey>;
  a2aKeys: Map<string, MockA2aKey>;
  a2aAgentsJson: string;
  configToml: string;
  hubConfigToml: string;
  providersToml: string;
  mcpJson: string;
  workspaceFiles: Record<string, MockWorkspaceEntry[]>;
  workspaceFileContents: Record<string, string>;
  inboxItems: UserInboxItem[];
  /**
   * Whether the agent has a conversation: the sample history and episodes
   * sit behind `extraRecent`. An agent has one once it has run.
   */
  hasConversation: boolean;
  sessions: MockSessions;
  /** Workbench artifacts: name → page HTML and modification time. */
  workbenchArtifacts: Map<string, MockArtifact>;
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
  /** A main turn is in progress. */
  busy: boolean;
  /** Main-conversation messages the web UI hasn't shown. */
  unread: number;
  state: MockState;
  /** How many web clients have this agent's WebSocket open. Set by the agent socket. */
  connectedClients: () => number;
}

/** An agent removed by `DELETE`, kept whole so a restore brings back its conversation and settings. */
export interface MockDeletedAgent {
  agent: MockAgent;
  deletedAt: string;
  checkpointId: string;
}

export interface MockHub {
  agents: Map<string, MockAgent>;
  /** Deleted agents that can be restored, by name. */
  deleted: Map<string, MockDeletedAgent>;
  /** Hub-level and team-level state: secrets, hub config, team files, the workbench. */
  hubState: MockState;
  /** Register an agent and open its WebSocket route. */
  createAgent: (
    name: string,
    options?: { role?: string | null; runState?: AgentState; lastError?: string },
  ) => MockAgent;
  summary: (agent: MockAgent) => AgentSummary;
  /** Send a frame to every hub WebSocket client. */
  broadcast: (frame: HubServerMessage) => void;
  setBusy: (agent: MockAgent, busy: boolean) => void;
  addUnread: (agent: MockAgent) => void;
  clearUnread: (agent: MockAgent) => void;
  /** Move an agent to a run state and tell hub clients. */
  transition: (agent: MockAgent, runState: AgentState) => void;
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

/**
 * Give an agent the data it has once it has run: a conversation, the sample
 * inbox and its A2A client settings. An agent that has never run has none of
 * it, and its file-only routes answer with empty data.
 */
export function seedAgentData(state: MockState): void {
  state.hasConversation = true;
  state.inboxItems = createInboxItems();
  state.a2aAgentsJson = SAMPLE_A2A_AGENTS_JSON;
}

/**
 * A fresh state with the sample data of an agent that has run, or with none
 * of it when `hasRun` is false.
 */
export function createState(agentName: string, hasRun = true): MockState {
  const state: MockState = {
    agentName,
    mode: process.env.VITE_MOCK_SETUP === "1" ? "setup" : "running",
    workbenchPort: null,
    workbenchArtifacts: createWorkbenchArtifacts(),
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
          created_at: new Date(Date.now() - 86400000 * 3).toISOString(),
        },
      ],
    ]),
    a2aAgentsJson: EMPTY_A2A_AGENTS_JSON,
    configToml: loadAsset("config.example.toml"),
    hubConfigToml: loadAsset("hub-config.example.toml"),
    providersToml: loadAsset("providers.example.toml"),
    mcpJson: loadAsset("mcp.example.json"),
    workspaceFiles: createWorkspaceFiles(),
    workspaceFileContents: createWorkspaceFileContents(),
    sessions: createSessions(),
    outboundTasks: [
      {
        task_id: "task-7f3a",
        agent: "research-buddy",
        sender_address: "main",
        state: "working",
        status_text: "Reading the three papers you linked and pulling out their benchmark numbers.",
        open: true,
        started_at: new Date(Date.now() - 4 * 60_000).toISOString(),
        unreachable_since: null,
      },
      {
        task_id: "task-19c2",
        agent: "laptop",
        sender_address: "main",
        state: "working",
        status_text: null,
        open: true,
        started_at: new Date(Date.now() - 42 * 60_000).toISOString(),
        unreachable_since: new Date(Date.now() - 17 * 60_000).toISOString(),
      },
    ],
    extraRecent: [],
    dropSockets: () => {},
    broadcast: () => {},
    compressedAt: null,
    inboxItems: [],
    hasConversation: false,
  };
  if (hasRun) seedAgentData(state);
  return state;
}
