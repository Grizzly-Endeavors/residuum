import type {
  A2aVisibility,
  AgentLastError,
  AgentSender,
  AgentListResponse,
  AgentState,
  AgentSummary,
  MessageSender,
  OutboundA2aTaskSummary,
  ServerMessage,
  SessionUsageTotals,
  TeamsSetupJob,
  TeamsSetupPrereqs,
  LogLine,
} from "../src/lib/generated/protocol";
import type { HubServerMessage } from "../src/lib/hub-types";
import type { RecentMessage, WorkspaceEntry } from "../src/lib/types";
import { loadAsset } from "./assets";
import { seedCheckpoints, type MockCheckpoints } from "./checkpoints";
import { seedChatAttachments } from "./data/chat";
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
import { emptyPairing, type MockPairing } from "./pairing";
import { defaultRemoteAccess, type MockRemoteAccess } from "./remote-access";
import type { MockUpdateStatus } from "./update";

export interface MockAgentKey {
  value: string;
  description: string;
  created_by: "user" | "agent";
}

export interface MockA2aKey {
  description: string;
  created_at: string;
}

export interface UserInboxAttachment {
  filename: string;
  mime_type: string;
  size: number;
  url: string;
}

export interface UserInboxItem {
  id: string;
  title: string;
  body: string;
  source: string;
  timestamp: string;
  read: boolean;
  attachments: UserInboxAttachment[];
  in_reply_to?: string;
}

export interface MockAgentInboxItem {
  id: string;
  title: string;
  body: string;
  source: string;
  timestamp: string;
  in_reply_to?: string;
  attachments?: UserInboxAttachment[];
  read?: boolean;
  archived?: boolean;
}

export interface MockUpload {
  id: string;
  filename: string;
  bytes: Buffer;
}

export interface MockState {
  agentName: string;
  env: MockEnv;
  mode: "setup" | "running";
  secrets: Map<string, string>;
  agentKeys: Map<string, MockAgentKey>;
  a2aKeys: Map<string, MockA2aKey>;
  a2aAgentsJson: string;
  configToml: string;
  hubConfigToml: string;
  providersToml: string;
  workspaceFiles: Record<string, WorkspaceEntry[]>;
  workspaceFileContents: Record<string, string>;
  inboxItems: UserInboxItem[];
  inboxArchive: UserInboxItem[];
  agentInbox: MockAgentInboxItem[];
  attachmentBytes: Map<string, Buffer>;
  uploads: Map<string, MockUpload>;
  scheduled: MockScheduled;
  update: MockUpdateStatus;
  cloud: MockCloud;
  pushDevices: MockPushDevice[];
  pairing: MockPairing;
  remoteAccess: MockRemoteAccess;
  checkpoints: MockCheckpoints;
  hasConversation: boolean;
  sessions: MockSessions;
  workbenchPort: number | null;
  extraRecent: RecentMessage[];
  usage: SessionUsageTotals;
  dropSockets: () => void;
  broadcast: (frame: ServerMessage) => void;
  journalOnly: (frame: ServerMessage) => void;
  messageMain: (content: string, sender: AgentSender) => void;
  outboundTasks: OutboundA2aTaskSummary[];
  compressedAt: number | null;
  teamsSetupJob?: TeamsSetupJob | null;
  teamsSetupPrereqs?: TeamsSetupPrereqs;
  teamsSetupAllLogs?: LogLine[];
}

export interface MockAgent {
  name: string;
  displayName: string;
  runState: AgentState;
  lastError: AgentLastError | null;
  autostart: boolean;
  role: string | null;
  visibility: A2aVisibility;
  busySince: string | null;
  stopping: boolean;
  unread: number;
  state: MockState;
  connectedClients: () => number;
  watchedPrefixes: () => string[];
  receiveMessage: (content: string, source: { endpoint: string; sender?: MessageSender }) => void;
  dispose: () => void;
}

export interface MockDeletedAgent {
  agent: MockAgent;
  deletedAt: string;
  checkpointId: string;
}

export interface MockHub {
  env: MockEnv;
  agents: Map<string, MockAgent>;
  deleted: Map<string, MockDeletedAgent>;
  appRebuilds: number;
  hubState: MockState;
  createAgent: (
    name: string,
    options?: {
      role?: string | null;
      displayName?: string;
      runState?: AgentState;
      lastError?: Omit<AgentLastError, "at">;
    },
  ) => MockAgent;
  summary: (agent: MockAgent) => AgentSummary;
  listing: () => AgentListResponse;
  broadcast: (frame: HubServerMessage) => void;
  relaySession: (agent: MockAgent, frame: SessionEventFrame) => void;
  lagSessionRelay: () => number;
  presentPushDevices: () => string[];
  teamEvents: MockTeamEvents;
  /** What Home shows about each agent: `GET /api/hub/overview` and the `agent_overview` frames. */
  overview: MockOverview;
  setBusy: (agent: MockAgent, busy: boolean) => void;
  markStopping: (agent: MockAgent) => void;
  reloadHubConfig: () => void;
  addUnread: (agent: MockAgent) => void;
  clearUnread: (agent: MockAgent) => void;
  transition: (agent: MockAgent, runState: AgentState) => void;
  setHubSocketOnline: (online: boolean) => void;
  reset: (options?: { setup?: boolean }) => void;
}

const SAMPLE_A2A_AGENTS_JSON =
  JSON.stringify(
    { agents: { "research-buddy": { url: "https://example.com/a2a/research-buddy" } } },
    null,
    2,
  ) + "\n";

const EMPTY_A2A_AGENTS_JSON = '{"agents":{}}';

export const MCP_JSON = "config/mcp.json";

export function seedAgentData(state: MockState): void {
  const { clock } = state.env;
  state.hasConversation = true;
  state.usage = {
    input_tokens: 412_880,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    output_tokens: 9_214,
    context_tokens: 18_402,
    tool_calls: 37,
  };
  state.inboxItems = createInboxItems(clock);
  state.inboxArchive = createArchivedInboxItems(clock);
  seedChatAttachments(state);
  state.a2aAgentsJson = SAMPLE_A2A_AGENTS_JSON;
  state.scheduled = createScheduled(clock);
}

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
    usage: {
      input_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      output_tokens: 0,
      context_tokens: null,
      tool_calls: 0,
    },
    dropSockets: () => {},
    broadcast: () => {},
    journalOnly: () => {},
    messageMain: () => {},
    compressedAt: null,
    inboxItems: [],
    inboxArchive: [],
    agentInbox: [],
    attachmentBytes: new Map(),
    uploads: new Map(),
    scheduled: { pulses: [], actions: [], fires: [], lastFires: {} },
    update: { latest: null, lastChecked: null },
    cloud: { tunnel: null, viaTunnel: false, signIns: [], signInsStarted: 0 },
    pushDevices: [],
    pairing: emptyPairing(),
    remoteAccess: defaultRemoteAccess(),
    checkpoints: {},
    hasConversation: false,
    teamsSetupJob: null,
    teamsSetupPrereqs: undefined,
    teamsSetupAllLogs: [],
  };
  if (hasRun) seedAgentData(state);
  seedCheckpoints(state);
  return state;
}
