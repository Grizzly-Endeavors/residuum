import type {
  AgentSender,
  ImageAttachment as _ImageAttachment,
  MessageAttachment as _MessageAttachment,
  MessageSender,
  SessionSummary as _SessionSummary,
  RepoStats as _RepoStats,
} from "./generated/protocol";
import type { AutoModeVerdict } from "./generated/AutoModeVerdict";
export type {
  ClientMessage,
  ServerMessage,
  TurnInProgress,
  SnapshotFrame,
  PostTurnActivityKind,
  MessageSender,
  AgentSender,
  TurnOrigin,
  Visibility,
  ImageAttachment,
  MessageAttachment,
  OutboundA2aTaskSummary,
  SessionCategory,
  SessionCommandErrorCode,
  SessionDeliveryOutcome,
  SessionListResponse,
  SessionRunStatus,
  SessionState,
  SessionSummary,
  SessionUsageTotals,
  WorkbenchInfo,
  WorkbenchRelayOrigins,
  ArtifactSummary,
  WorkspaceChange,
  WorkspaceChangeKind,
  WorkspaceResyncReason,
  CheckpointSummary,
  CheckpointPage,
  CheckpointDetail,
  ChangedPath,
  ChangeKind,
  RepoKind,
  RepoStats,
  RestoreOutcome,
  UndoOutcome,
  CheckpointTrigger,
  PulseFirePhase,
  PulseOverlap,
  PulseInfo,
  ActionInfo,
  ScheduledCurrentRun,
  ScheduledRunOutcome,
  ToolStatus,
  AtkStatus,
  SuggestedEndpointSource,
  TeamsSetupPrereqs,
  TeamsSetupForm,
  TeamsSetupStart,
  TeamsSetupPhase,
  TeamsSetupState,
  LogStream,
  LogLine,
  SignInPrompt,
  SetupError,
  CreatedResources,
  SetupResult,
  TeamsSetupJob,
  TeamsSetupRedirect,
  CleanupRequest,
  CleanupFailure,
  CleanupResult,
} from "./generated/protocol";

type ImageAttachment = _ImageAttachment;
type MessageAttachment = _MessageAttachment;
type SessionSummary = _SessionSummary;
type RepoStats = _RepoStats;

export interface ToolCallRecord {
  id: string;
  name: string;
  arguments: unknown;
  server?: string | null;
}

export interface SentAttachment {
  filename: string;
  mime_type: string;
  size: number;
  url: string;
  caption?: string | null;
}

export interface RecentMessage {
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  tool_calls?: ToolCallRecord[];
  tool_call_id?: string;
  thinking?: string[];
  timestamp: string;
  visibility: "user" | "background";
  images?: ImageAttachment[];
  attachments?: MessageAttachment[];
  sent_attachment?: SentAttachment;
  sender?: MessageSender;
  agent_sender?: AgentSender;
  turn_id?: string;
  id?: string;
  context?: boolean;
}

/** `GET /api/sessions/runs/{run_id}/transcript` response. */
export interface SessionTranscriptResponse {
  session: SessionSummary;
  messages: RecentMessage[];
}

export interface RecentHistorySegment {
  kind: "recent";
  messages: RecentMessage[];
  next_cursor: string | null;
}

export interface EpisodeHistorySegment {
  kind: "episode";
  episode_id: string;
  date: string; // YYYY-MM-DD
  messages: RecentMessage[];
  next_cursor: string | null;
}

export type ChatHistorySegment = RecentHistorySegment | EpisodeHistorySegment;

export interface StatusResponse {
  mode: "setup" | "running";
  version: string;
  features: string[];
  checkpoints: {
    workspace: RepoStats | null;
    team: RepoStats | null;
    agent_config: RepoStats | null;
    hub: RepoStats | null;
  };
}

export type ProviderKey = "anthropic" | "openai" | "gemini" | "fireworks" | "ollama";

export interface ProviderConfig {
  apiKey: string;
  model: string;
  url: string;
}

export interface RoleConfig {
  provider: string;
  url: string;
  model: string;
}

export interface EmbeddingConfig {
  provider: string;
  model: string;
}

export interface BackgroundModelConfig {
  provider: string;
  model: string;
}

export interface McpServerEntry {
  name: string;
  transport?: "stdio" | "http";
  command: string;
  args: string[];
  env: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
}

export interface McpRequiredInput {
  field: string;
  label: string;
}

export interface McpCatalogEntry {
  name: string;
  description: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  category: string;
  requires_input: McpRequiredInput[];
  install_hint: string;
}

export interface IntegrationsConfig {
  discordToken: string;
  telegramToken: string;
  teamsAppId: string;
  teamsTenantId: string;
  teamsAppPassword: string;
}

export interface SetupMcpServer extends McpServerEntry {
  secretEnvKeys?: string[];
}

export interface SetupWizardState {
  userName: string;
  agentName: string;
  timezone: string;
  selectedProviders: ProviderKey[];
  providerConfigs: Record<ProviderKey, ProviderConfig>;
  mainProvider: ProviderKey;
  roles: Record<string, RoleConfig>;
  embeddingModel: EmbeddingConfig;
  backgroundModels: Record<string, BackgroundModelConfig>;
  mcpServers: SetupMcpServer[];
  integrations: IntegrationsConfig;
  secretRefs: Record<string, string>;
}

export interface TimezoneResponse {
  timezone: string;
}

export interface ModelsResponse {
  models: { id: string; name: string }[];
  error?: string;
}

export interface SecretResponse {
  reference: string;
}

export type DiagnosticSeverity = "error" | "warning";

export type DiagnosticLocation =
  | { kind: "line"; line: number }
  | { kind: "line_column"; line: number; column: number }
  | { kind: "path"; path: string };

export interface Diagnostic {
  severity: DiagnosticSeverity;
  message: string;
  location?: DiagnosticLocation;
}

export interface ValidateResponse {
  valid: boolean;
  error?: string;
  diagnostics?: Diagnostic[];
  checkpoint_id?: string;
}

export interface RollbackNoticeResponse {
  attempted_version: string;
  reason: string;
  at: string;
}

export interface UnverifiedUpdateResponse {
  version: string;
  at: string;
}

export interface UpdateStatusResponse {
  current: string;
  latest: string | null;
  update_available: boolean;
  last_checked: string | null;
  checking: boolean;
  rollback_notice: RollbackNoticeResponse | null;
  unverified_update: UnverifiedUpdateResponse | null;
}

export type CloudTunnelStatus = "disconnected" | "connecting" | "refused" | "connected";

export interface CloudStatusResponse {
  status: CloudTunnelStatus;
  user_id: string | null;
  has_token: boolean;
  enabled: boolean;
  viewed_via_tunnel: boolean;
  origin?: string | null;
  instance?: string | null;
  instance_origin?: string;
}

export interface SettingsProviderEntry {
  name: string;
  type: string;
  apiKey: string;
  url: string;
  keepAlive: string;
  models: SettingsProviderModel[];
}

export type ModelImageInput = "" | "image" | "text";

export interface SettingsProviderModel {
  name: string;
  images: ModelImageInput;
}

export interface RoleOverrides {
  temperature: string;
  thinking: string;
}

export type ModelRoleKey =
  | "main"
  | "default"
  | "observer"
  | "reflector"
  | "pulse"
  | "subconscious"
  | "embedding"
  | "bgSmall"
  | "bgMedium"
  | "bgLarge";

export interface SettingsModelAssignments {
  main: string;
  default: string;
  observer: string;
  reflector: string;
  pulse: string;
  subconscious: string;
  embedding: string;
  bgSmall: string;
  bgMedium: string;
  bgLarge: string;
  overrides: Record<string, RoleOverrides>;
  fallbacks: Record<string, string[]>;
}

export interface AgentKeyInfo {
  name: string;
  env_var: string;
  description: string;
  created_by: "user" | "agent";
}

export interface AgentKeysListResponse {
  keys: AgentKeyInfo[];
}

export interface SetAgentKeyResponse {
  name: string;
  env_var: string;
  warning?: string;
}

export interface SecretsListResponse {
  names: string[];
}

export interface DeleteSecretResponse {
  deleted: boolean;
}

export interface A2aStatusResponse {
  enabled: boolean;
  port: number;
  visibility: "public" | "private";
  public_url: string | null;
  local_url: string;
  relay_access: boolean;
  relay_access_note: string;
  listener_running: boolean;
  card_error: string | null;
}

export interface A2aKeyInfo {
  name: string;
  description: string;
  created_at: string;
}

export interface A2aKeysListResponse {
  keys: A2aKeyInfo[];
}

export interface CreateA2aKeyResponse {
  name: string;
  token: string;
}

export interface A2aCardSkill {
  id: string;
  name: string;
  description?: string;
}

export interface A2aAgentCard {
  name: string;
  description: string;
  skills: A2aCardSkill[];
}

export type A2aAgentStatus = "pending" | "ok" | "error";

export interface A2aRemoteAgent {
  name: string;
  url: string;
  source: "config" | "sibling";
  status: A2aAgentStatus;
  error: string | null;
  card: A2aAgentCard | null;
}

export interface WorkspaceEntry {
  name: string;
  entry_type: "file" | "directory";
  size: number | null;
  modified: number;
  version: string;
}

/** Response from `PUT /api/workspace/file` and `PUT /api/workspace/raw`.
 * `saved` is always `true` — an invalid strictly-parsed file (HEARTBEAT.yml,
 * say) is still written; `diagnostics` names what's wrong with it instead of
 * the save being rejected. */
export interface WorkspaceWriteResponse {
  saved: boolean;
  version: string;
  diagnostics?: Diagnostic[];
}

/** Response from `POST /api/workspace/move`. */
export interface WorkspaceMoveResponse {
  moved: boolean;
  version: string | null;
  diagnostics?: Diagnostic[];
}

export interface WorkspaceValidateResponse {
  diagnostics: Diagnostic[];
}

export type ConnectionStatus = "connecting" | "connected" | "disconnected";

export interface ToolCallState {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
  status: "running" | "done" | "error" | "stopped";
  result?: string;
  server?: string | null;
  autoMode?: AutoModeVerdict;
  startedAt?: number;
  endedAt?: number;
}

interface FeedItemBase {
  id: number;
  turnId?: string;
  midTurn?: boolean;
  timestamp?: string;
}

export interface TurnRef {
  turnId: string;
  changed: boolean | null;
}

export interface UserFeedItem extends FeedItemBase {
  kind: "user";
  content: string;
  images?: ImageAttachment[];
  attachments?: MessageAttachment[];
  sender?: MessageSender;
  turn?: TurnRef;
  messageId?: string;
}

export interface WaitingMessage {
  id: string;
  content: string;
  images?: ImageAttachment[];
  attachments?: MessageAttachment[];
  sender?: MessageSender;
  endpoint: string;
  connection: number;
  asked?: "send-now" | "withdraw";
}

export interface AssistantFeedItem extends FeedItemBase {
  kind: "assistant";
  content: string;
  call?: number;
  streaming?: boolean;
  cut?: "stopped" | "interrupted";
  deliveredTo?: string;
}

export interface ThinkingFeedItem extends FeedItemBase {
  kind: "thinking";
  content: string;
  call?: number;
  streaming?: boolean;
  /** `Date.now()` when the page saw it start, and when it ended; history doesn't keep them. */
  startedAt?: number;
  endedAt?: number;
}

export interface DividerFeedItem extends FeedItemBase {
  kind: "divider";
  label: string;
  variant?: "episode" | "day";
  date?: string;
  episode?: string;
}

export interface CompressedMarkerFeedItem extends FeedItemBase {
  kind: "compressed-marker";
}

export interface ToolGroupFeedItem extends FeedItemBase {
  kind: "tool-group";
  calls: ToolCallState[];
  call?: number;
  closed?: true;
}

export interface FileAttachmentFeedItem extends FeedItemBase {
  kind: "file-attachment";
  filename: string;
  mimeType: string;
  size: number;
  url: string;
  caption: string | null;
}

export interface LocalSystemFeedItem extends FeedItemBase {
  kind: "local-system";
  content: string;
}

export interface AgentMessageFeedItem extends FeedItemBase {
  kind: "agent-message";
  from: string;
  category: string | null;
  content: string;
  runId: string | null;
  messageId?: string;
}

export interface StatusFeedItem extends FeedItemBase {
  kind: "status";
  tone: "info" | "error";
  content: string;
  details?: string;
}

export interface TurnFailureFeedItem extends FeedItemBase {
  kind: "turn-failure";
  turnId: string;
  message: string;
  details?: string;
  retry?: TurnRetry;
}

export interface TurnRetry {
  asked?: true;
  refused?: string;
}

export type FeedItem =
  | ThinkingFeedItem
  | TurnFailureFeedItem
  | AgentMessageFeedItem
  | StatusFeedItem
  | UserFeedItem
  | AssistantFeedItem
  | DividerFeedItem
  | CompressedMarkerFeedItem
  | ToolGroupFeedItem
  | FileAttachmentFeedItem
  | LocalSystemFeedItem;
