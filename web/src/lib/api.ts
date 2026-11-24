import type {
  StatusResponse,
  ChatHistorySegment,
  RecentHistorySegment,
  EpisodeHistorySegment,
  TimezoneResponse,
  ModelsResponse,
  McpCatalogEntry,
  SecretResponse,
  ValidateResponse,
  SecretsListResponse,
  AgentKeyInfo,
  AgentKeysListResponse,
  SetAgentKeyResponse,
  A2aStatusResponse,
  A2aAgentCard,
  A2aKeyInfo,
  A2aKeysListResponse,
  CreateA2aKeyResponse,
  A2aRemoteAgent,
  OutboundA2aTaskSummary,
  WorkspaceEntry,
  WorkspaceWriteResponse,
  WorkspaceValidateResponse,
  Diagnostic,
  CloudStatusResponse,
  UpdateStatusResponse,
  SessionCategory,
  SessionDeliveryOutcome,
  SessionListResponse,
  SessionTranscriptResponse,
  SessionUsageTotals,
  WorkbenchInfo,
  ArtifactSummary,
  PulseInfo,
  ActionInfo,
  CheckpointPage,
  CheckpointDetail,
  RepoStats,
  RestoreOutcome,
  UndoOutcome,
  RepoKind,
  CleanupRequest,
  CleanupResult,
  TeamsSetupJob,
  TeamsSetupPrereqs,
  TeamsSetupStart,
} from "./types";
import type {
  A2aVisibility,
  AgentListResponse,
  AgentSummary,
  CreateAgentRequest,
  DeleteOutcome,
  DeletedAgent,
  DeletedAgentListResponse,
  RestoreAgentRequest,
  AgentPatch,
  HubStatusResponse,
  MailFolder,
  MailReplyResponse,
  MailSendResponse,
  MailThread,
  MailThreadPage,
  OverviewResponse,
  PushDevice,
  PushPreferencesPatch,
  PushTestResult,
  TeamEventPage,
  WebPushSubscription,
  WorkspaceScope,
} from "./hub-types";
import type { PushDeviceList } from "./generated/PushDeviceList";
import type { PushDeviceResponse } from "./generated/PushDeviceResponse";
import type { PushKeyResponse } from "./generated/PushKeyResponse";
import { cachedFetch, invalidate } from "./cache";
import { agentBase, agentPath, hubPath, requireAgent, teamPath } from "./paths";
import { isDeviceRequiredBody, redirectToPairing } from "./pairing";

export const CACHE_KEY_TIMEZONE = `GET ${hubPath("/system/timezone")}`;
export const CACHE_KEY_MCP_CATALOG = `GET ${hubPath("/mcp-catalog")}`;
export const CACHE_KEY_HUB_CONFIG_RAW = `GET ${hubPath("/config/raw")}`;

function agentCacheKey(agent: string, sub: string): string {
  return `GET ${agentBase(agent)}${sub}`;
}

export const cacheKeyStatus = (agent: string): string => agentCacheKey(agent, "/status");
export const cacheKeyConfigRaw = (agent: string): string => agentCacheKey(agent, "/config/raw");
export const cacheKeyProvidersRaw = (agent: string): string =>
  agentCacheKey(agent, "/providers/raw");
export const cacheKeyMcpRaw = (agent: string): string => agentCacheKey(agent, "/mcp/raw");

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly statusText: string,
    public readonly body: string,
  ) {
    super(`${status} ${statusText}: ${body}`);
    this.name = "ApiError";
  }
}

export function validationFromApiError(err: unknown): ValidateResponse | null {
  if (!(err instanceof ApiError) || err.status !== 400) return null;
  try {
    const parsed: unknown = JSON.parse(err.body);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      "error" in parsed &&
      typeof parsed.error === "string"
    ) {
      return { valid: false, error: parsed.error };
    }
  } catch {
    return null;
  }
  return null;
}

export interface WorkspaceWriteConflict {
  error: string;
  currentVersion: string | null;
}

export function workspaceConflictFromApiError(err: unknown): WorkspaceWriteConflict | null {
  if (!(err instanceof ApiError) || err.status !== 412) return null;
  try {
    const parsed: unknown = JSON.parse(err.body);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      "error" in parsed &&
      typeof parsed.error === "string" &&
      "current_version" in parsed &&
      (typeof parsed.current_version === "string" || parsed.current_version === null)
    ) {
      return { error: parsed.error, currentVersion: parsed.current_version };
    }
  } catch {
    return null;
  }
  return null;
}

async function putValidated(
  path: string,
  contentType: string,
  body: string,
  cacheKey: string | null,
): Promise<ValidateResponse> {
  try {
    return await apiFetch<ValidateResponse>(path, {
      method: "PUT",
      headers: { "Content-Type": contentType },
      body,
    });
  } catch (err: unknown) {
    const validation = validationFromApiError(err);
    if (validation) return validation;
    throw err;
  } finally {
    if (cacheKey !== null) invalidate(cacheKey);
  }
}

async function patchValidated(
  path: string,
  diff: Record<string, unknown>,
  cacheKey: string,
): Promise<ValidateResponse> {
  if (Object.keys(diff).length === 0) {
    return { valid: true, error: undefined };
  }
  try {
    return await apiFetch<ValidateResponse>(path, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(diff),
    });
  } catch (err: unknown) {
    const validation = validationFromApiError(err);
    if (validation) return validation;
    throw err;
  } finally {
    invalidate(cacheKey);
  }
}

async function checkOk(resp: Response): Promise<Response> {
  if (!resp.ok) {
    const body = await resp.text();
    // Through Residuum Cloud, a browser the gateway hasn't paired is sent to
    // the pairing page; the call still fails for whoever made it.
    if (resp.status === 401 && isDeviceRequiredBody(body)) redirectToPairing();
    throw new ApiError(resp.status, resp.statusText, body);
  }
  return resp;
}

async function apiFetch<T>(input: RequestInfo | URL, init?: RequestInit): Promise<T> {
  const resp = await checkOk(await fetch(input, init));
  return (await resp.json()) as T;
}

async function apiFetchText(input: RequestInfo | URL, init?: RequestInit): Promise<string> {
  const resp = await checkOk(await fetch(input, init));
  return resp.text();
}

export async function fetchStatus(agent: string): Promise<StatusResponse> {
  return cachedFetch(cacheKeyStatus(agent), () =>
    apiFetch<StatusResponse>(agentPath(agent, "/status")),
  );
}

export async function fetchChatHistory(agent: string): Promise<RecentHistorySegment> {
  const segment = await apiFetch<ChatHistorySegment>(agentPath(agent, "/chat/history"));
  if (segment.kind === "recent") return segment;
  throw new Error(
    `unexpected chat history kind "${segment.kind}" — server must return Recent for the base call`,
  );
}

export async function fetchUsageTotals(agent: string): Promise<SessionUsageTotals> {
  return apiFetch<SessionUsageTotals>(agentPath(agent, "/usage"));
}

export async function fetchChatSegment(
  agent: string,
  episodeId: string,
): Promise<EpisodeHistorySegment> {
  const segment = await apiFetch<ChatHistorySegment>(
    agentPath(agent, `/chat/history?episode=${encodeURIComponent(episodeId)}`),
  );
  if (segment.kind === "episode") return segment;
  throw new Error(
    `unexpected chat history kind "${segment.kind}" — server must return Episode for ?episode=`,
  );
}

export interface FeedbackReceipt {
  public_id: string;
  submitted_at: string;
}

export type BugSeverity = "broken" | "wrong" | "annoying";

export async function submitBugReport(body: {
  what_happened: string;
  what_expected: string;
  what_doing: string;
  severity: BugSeverity;
}): Promise<FeedbackReceipt> {
  return apiFetch<FeedbackReceipt>(hubPath("/tracing/bug-report"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export async function submitFeedback(body: {
  message: string;
  category?: string;
}): Promise<FeedbackReceipt> {
  return apiFetch<FeedbackReceipt>(hubPath("/tracing/feedback"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export async function fetchTimezone(): Promise<string> {
  try {
    const data = await cachedFetch(CACHE_KEY_TIMEZONE, () =>
      apiFetch<TimezoneResponse>(hubPath("/system/timezone")),
    );
    return data.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  }
}

export async function fetchProviderModels(
  agent: string | null,
  provider: string,
  apiKey?: string,
  url?: string,
): Promise<ModelsResponse> {
  const body: Record<string, string> = { provider };
  if (apiKey) body.api_key = apiKey;
  if (url) body.url = url;

  const path =
    agent === null ? hubPath("/providers/models") : agentPath(agent, "/providers/models");
  return apiFetch<ModelsResponse>(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export interface SystemOneForm {
  provider: string;
  url?: string;
  model?: string;
  api_key?: string;
  keep_alive?: string;
}

/** `POST /api/hub/system-one/models`. */
export interface SystemOneModelsResponse {
  models: { id: string; description: string | null }[];
  error?: string;
}

export interface SystemOneTestResponse {
  ok: boolean;
  message: string;
  answered_by?: string;
}

/** The model names a decision model provider accepts. Throws `ApiError` only when the hub can't be reached. */
export async function fetchSystemOneModels(form: SystemOneForm): Promise<SystemOneModelsResponse> {
  return apiFetch<SystemOneModelsResponse>(hubPath("/system-one/models"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(form),
  });
}

export async function testSystemOne(form: SystemOneForm): Promise<SystemOneTestResponse> {
  return apiFetch<SystemOneTestResponse>(hubPath("/system-one/test"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(form),
  });
}

export async function fetchMcpCatalog(): Promise<McpCatalogEntry[]> {
  return cachedFetch(CACHE_KEY_MCP_CATALOG, () =>
    apiFetch<McpCatalogEntry[]>(hubPath("/mcp-catalog")),
  );
}

export async function storeSecret(name: string, value: string): Promise<SecretResponse> {
  return apiFetch<SecretResponse>(hubPath("/secrets"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, value }),
  });
}

export interface CompleteSetupPayload {
  hubConfig: string;
  agentName: string;
  userName: string;
  config: string;
  providers: string;
  mcpJson?: string;
}

export async function completeSetup(setup: CompleteSetupPayload): Promise<ValidateResponse> {
  const payload: Record<string, string> = {
    hub_config: setup.hubConfig,
    agent_name: setup.agentName,
    config: setup.config,
    providers: setup.providers,
  };
  if (setup.userName) payload.user_name = setup.userName;
  if (setup.mcpJson) payload.mcp_json = setup.mcpJson;
  return apiFetch<ValidateResponse>(hubPath("/config/complete-setup"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
}

export async function fetchConfigRaw(agent: string): Promise<string> {
  return cachedFetch(cacheKeyConfigRaw(agent), () => apiFetchText(agentPath(agent, "/config/raw")));
}

export async function putConfigRaw(agent: string, toml: string): Promise<ValidateResponse> {
  return putValidated(
    agentPath(agent, "/config/raw"),
    "text/plain",
    toml,
    cacheKeyConfigRaw(agent),
  );
}

export async function patchConfig(
  agent: string,
  diff: Record<string, unknown>,
): Promise<ValidateResponse> {
  return patchValidated(agentPath(agent, "/config/patch"), diff, cacheKeyConfigRaw(agent));
}

export async function validateConfig(agent: string, toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>(agentPath(agent, "/config/validate"), {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchHubConfigRaw(): Promise<string> {
  return cachedFetch(CACHE_KEY_HUB_CONFIG_RAW, () => apiFetchText(hubPath("/config/raw")));
}

export async function putHubConfigRaw(toml: string): Promise<ValidateResponse> {
  return putValidated(hubPath("/config/raw"), "text/plain", toml, CACHE_KEY_HUB_CONFIG_RAW);
}

export async function patchHubConfig(diff: Record<string, unknown>): Promise<ValidateResponse> {
  return patchValidated(hubPath("/config/patch"), diff, CACHE_KEY_HUB_CONFIG_RAW);
}

export async function validateHubConfig(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>(hubPath("/config/validate"), {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchProvidersRaw(agent: string): Promise<string> {
  return cachedFetch(cacheKeyProvidersRaw(agent), () =>
    apiFetchText(agentPath(agent, "/providers/raw")),
  );
}

export async function putProvidersRaw(agent: string, toml: string): Promise<ValidateResponse> {
  return putValidated(
    agentPath(agent, "/providers/raw"),
    "text/plain",
    toml,
    cacheKeyProvidersRaw(agent),
  );
}

export async function patchProviders(
  agent: string,
  diff: Record<string, unknown>,
): Promise<ValidateResponse> {
  return patchValidated(agentPath(agent, "/providers/patch"), diff, cacheKeyProvidersRaw(agent));
}

export async function validateProviders(agent: string, toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>(agentPath(agent, "/providers/validate"), {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchMcpRaw(agent: string): Promise<string> {
  return cachedFetch(cacheKeyMcpRaw(agent), () => apiFetchText(agentPath(agent, "/mcp/raw")));
}

export async function putMcpRaw(agent: string, json: string): Promise<ValidateResponse> {
  return putValidated(
    agentPath(agent, "/mcp/raw"),
    "application/json",
    json,
    cacheKeyMcpRaw(agent),
  );
}

export async function validateMcp(agent: string, json: string): Promise<Diagnostic[]> {
  const result = await apiFetch<WorkspaceValidateResponse>(
    agentPath(agent, "/workspace/validate"),
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ path: "config/mcp.json", content: json }),
    },
  );
  return result.diagnostics;
}

export async function patchMcp(
  agent: string,
  diff: Record<string, unknown>,
): Promise<ValidateResponse> {
  return patchValidated(agentPath(agent, "/mcp/patch"), diff, cacheKeyMcpRaw(agent));
}

export async function fetchSecretNames(): Promise<string[]> {
  const data = await apiFetch<SecretsListResponse>(hubPath("/secrets"));
  return data.names;
}

export async function listSecrets(): Promise<string[]> {
  try {
    return await fetchSecretNames();
  } catch {
    return [];
  }
}

export async function deleteSecret(name: string): Promise<void> {
  await apiFetchText(hubPath(`/secrets/${encodeURIComponent(name)}`), {
    method: "DELETE",
  });
}

export async function fetchAgentKeys(): Promise<AgentKeyInfo[]> {
  const data = await apiFetch<AgentKeysListResponse>(hubPath("/agent-keys"));
  return data.keys;
}

export async function storeAgentKey(
  name: string,
  value: string,
  description: string,
): Promise<SetAgentKeyResponse> {
  return apiFetch<SetAgentKeyResponse>(hubPath("/agent-keys"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, value, description }),
  });
}

export async function deleteAgentKey(name: string): Promise<string | null> {
  return readCheckpointId(hubPath(`/agent-keys/${encodeURIComponent(name)}`), { method: "DELETE" });
}

export async function fetchA2aStatus(agent: string): Promise<A2aStatusResponse> {
  return apiFetch<A2aStatusResponse>(agentPath(agent, "/a2a/status"));
}

/**
 * The Agent Card as currently served. Throws `ApiError` — including a `503`
 * (`status` on the error) when the workspace agent card file is invalid,
 * whose body is the plain-language reason.
 */
export async function fetchA2aCard(agent: string): Promise<A2aAgentCard> {
  return apiFetch<A2aAgentCard>(agentPath(agent, "/a2a/card"));
}

export async function fetchA2aKeys(): Promise<A2aKeyInfo[]> {
  const data = await apiFetch<A2aKeysListResponse>(hubPath("/a2a/keys"));
  return data.keys;
}

export async function createA2aKey(
  name: string,
  description: string,
): Promise<CreateA2aKeyResponse> {
  return apiFetch<CreateA2aKeyResponse>(hubPath("/a2a/keys"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, description: description || undefined }),
  });
}

export async function revokeA2aKey(name: string): Promise<string | null> {
  return readCheckpointId(hubPath(`/a2a/keys/${encodeURIComponent(name)}`), { method: "DELETE" });
}

export async function fetchA2aAgents(agent: string): Promise<A2aRemoteAgent[]> {
  return apiFetch<A2aRemoteAgent[]>(agentPath(agent, "/a2a/agents"));
}

export async function fetchOutboundA2aTasks(agent: string): Promise<OutboundA2aTaskSummary[]> {
  return apiFetch<OutboundA2aTaskSummary[]>(agentPath(agent, "/a2a/outbound"));
}

/**
 * Ask a task's remote agent to cancel it. Throws `ApiError`: `404` when the
 * task already ended, `502` when its agent can't be reached (then
 * `stopWatchingOutboundA2aTask` is the way out).
 */
export async function stopOutboundA2aTask(
  agent: string,
  taskId: string,
): Promise<OutboundA2aTaskSummary> {
  return apiFetch<OutboundA2aTaskSummary>(
    agentPath(agent, `/a2a/outbound/${encodeURIComponent(taskId)}/stop`),
    {
      method: "POST",
    },
  );
}

export async function stopWatchingOutboundA2aTask(
  agent: string,
  taskId: string,
): Promise<OutboundA2aTaskSummary> {
  return apiFetch<OutboundA2aTaskSummary>(
    agentPath(agent, `/a2a/outbound/${encodeURIComponent(taskId)}/stop-watching`),
    { method: "POST" },
  );
}

export async function fetchA2aAgentsRaw(agent: string): Promise<string> {
  return apiFetchText(agentPath(agent, "/a2a/agents/raw"));
}

export async function putA2aAgentsRaw(agent: string, content: string): Promise<ValidateResponse> {
  return putValidated(agentPath(agent, "/a2a/agents/raw"), "application/json", content, null);
}

export async function fetchSessions(
  agent: string,
  query: {
    category?: SessionCategory;
    before?: string;
    limit?: number;
    address?: string;
    artifact?: string;
  },
): Promise<SessionListResponse> {
  const params = new URLSearchParams();
  if (query.category) params.set("category", query.category);
  if (query.before) params.set("before", query.before);
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  if (query.address) params.set("address", query.address);
  if (query.artifact) params.set("artifact", query.artifact);
  const qs = params.toString();
  return apiFetch<SessionListResponse>(agentPath(agent, `/sessions${qs ? `?${qs}` : ""}`));
}

export async function fetchSessionTranscript(
  agent: string,
  runId: string,
): Promise<SessionTranscriptResponse> {
  return apiFetch<SessionTranscriptResponse>(
    agentPath(agent, `/sessions/runs/${encodeURIComponent(runId)}/transcript`),
  );
}

export type SessionModelSize = "small" | "medium" | "large";

export interface StartSessionRequest {
  prompt: string;
  model?: SessionModelSize;
  fork?: boolean;
}

/**
 * Start a session of the owner's own on `agent`: a clean one, or with `fork`
 * a fork of the main conversation. Resolves to its address as soon as the
 * agent takes the request; the run announces itself with `session_started`.
 *
 * Throws `ApiError` with `{ error }`: `400` for a blank prompt or a bad
 * model, `500` when a fork couldn't read the main conversation, `503` while
 * Residuum shuts down.
 */
export async function startSession(agent: string, request: StartSessionRequest): Promise<string> {
  const reply = await apiFetch<{ address: string }>(agentPath(agent, "/sessions"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
  return reply.address;
}

/** `POST /api/sessions/{address}/messages` response. */
export interface SessionMessageReply {
  outcome: SessionDeliveryOutcome;
  id: string;
}

/**
 * Send the session at `address` a message as the owner, under `id`:
 * delivered to a live run, or starting a new run of a finished one. Resolves
 * to where it landed.
 *
 * Throws `ApiError` with `{ error, code }`: `404` for an unknown address,
 * `409` when the session is busy, `502` when delivery failed.
 */
export async function messageSession(
  agent: string,
  address: string,
  content: string,
  id: string,
): Promise<SessionMessageReply> {
  return apiFetch<SessionMessageReply>(
    agentPath(agent, `/sessions/${encodeURIComponent(address)}/messages`),
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ content, id }),
    },
  );
}

export async function stopSession(agent: string, address: string): Promise<void> {
  await apiFetch<unknown>(agentPath(agent, `/sessions/${encodeURIComponent(address)}/stop`), {
    method: "POST",
  });
}

export async function fetchScheduledPulses(agent: string): Promise<PulseInfo[]> {
  return apiFetch<PulseInfo[]>(agentPath(agent, "/scheduled/pulses"));
}

export async function setPulseEnabled(
  agent: string,
  name: string,
  enabled: boolean,
): Promise<void> {
  await apiFetch<unknown>(
    agentPath(agent, `/scheduled/pulses/${encodeURIComponent(name)}/enabled`),
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ enabled }),
    },
  );
}

export async function stopPulseFire(agent: string, pulse: string, fireId: string): Promise<void> {
  await apiFetch<unknown>(
    agentPath(
      agent,
      `/scheduled/pulses/${encodeURIComponent(pulse)}/fires/${encodeURIComponent(fireId)}/stop`,
    ),
    { method: "POST" },
  );
}

export async function fetchScheduledActions(agent: string): Promise<ActionInfo[]> {
  return apiFetch<ActionInfo[]>(agentPath(agent, "/scheduled/actions"));
}

export async function cancelScheduledAction(agent: string, id: string): Promise<void> {
  await apiFetch<unknown>(agentPath(agent, `/scheduled/actions/${encodeURIComponent(id)}`), {
    method: "DELETE",
  });
}

export async function fetchWorkbenchArtifacts(): Promise<ArtifactSummary[]> {
  return apiFetch<ArtifactSummary[]>(teamPath("/workbench/artifacts"));
}

export async function fetchWorkbenchInfo(): Promise<WorkbenchInfo> {
  return apiFetch<WorkbenchInfo>(teamPath("/workbench/info"));
}

export interface ArtifactDeletion {
  paths: string[];
  checkpointId: string | null;
}

export async function deleteWorkbenchArtifact(name: string): Promise<ArtifactDeletion> {
  const body = await apiFetch<{ removed?: unknown; checkpoint_id?: unknown }>(
    teamPath(`/workbench/artifacts/${encodeURIComponent(name)}`),
    { method: "DELETE" },
  );
  const removed = Array.isArray(body.removed)
    ? body.removed.filter((entry): entry is string => typeof entry === "string")
    : [];
  return {
    // A folder artifact is reported as `name/`; the checkpoint stores it as `name`.
    paths: removed.map((entry) => `workbench/${entry.replace(/\/$/, "")}`),
    checkpointId:
      typeof body.checkpoint_id === "string" && body.checkpoint_id.length > 0
        ? body.checkpoint_id
        : null,
  };
}

function workspacePath(agent: string | null, sub: string, scope: WorkspaceScope): string {
  return scope === "team"
    ? teamPath(`/workspace${sub}`)
    : agentPath(requireAgent(agent), `/workspace${sub}`);
}

export async function fetchWorkspaceFiles(
  agent: string | null,
  path?: string,
  scope: WorkspaceScope = "agent",
): Promise<WorkspaceEntry[]> {
  const params = path ? `?path=${encodeURIComponent(path)}` : "";
  return apiFetch<WorkspaceEntry[]>(workspacePath(agent, `/files${params}`, scope));
}

export interface WorkspaceFileRead {
  content: string;
  version: string;
}

export async function fetchWorkspaceFile(
  agent: string | null,
  path: string,
  scope: WorkspaceScope = "agent",
): Promise<WorkspaceFileRead> {
  const resp = await checkOk(
    await fetch(workspacePath(agent, `/file?path=${encodeURIComponent(path)}`, scope)),
  );
  return { content: await resp.text(), version: resp.headers.get("etag") ?? "" };
}

/**
 * Writes with `If-Match: version` so a concurrent edit (the agent, another
 * tab) is never silently overwritten: a stale version throws `ApiError`
 * with status `412` — see `workspaceConflictFromApiError`. Pass `null` only
 * for a brand-new file that has no version yet. `diagnostics` on the
 * response names what's wrong with an invalid strictly-parsed file
 * (HEARTBEAT.yml, say) — the write still succeeds either way.
 */
export async function putWorkspaceFile(
  agent: string | null,
  path: string,
  content: string,
  version: string | null,
  scope: WorkspaceScope = "agent",
): Promise<WorkspaceWriteResponse> {
  return apiFetch<WorkspaceWriteResponse>(workspacePath(agent, "/file", scope), {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      ...(version ? { "If-Match": version } : {}),
    },
    body: JSON.stringify({ path, content }),
  });
}

export async function validateWorkspaceFile(
  agent: string | null,
  path: string,
  content: string,
  scope: WorkspaceScope = "agent",
): Promise<Diagnostic[]> {
  try {
    const result = await apiFetch<WorkspaceValidateResponse>(
      workspacePath(agent, "/validate", scope),
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path, content }),
      },
    );
    return result.diagnostics;
  } catch {
    return [];
  }
}

export interface WorkspaceCheckpoint {
  id: string;
  repo: RepoKind;
}

interface CheckpointFields {
  checkpoint_id?: unknown;
  checkpoint_repo?: unknown;
}

export function parseWorkspaceCheckpoints(
  body: CheckpointFields & { checkpoints?: unknown },
): WorkspaceCheckpoint[] {
  const repoOf = (value: unknown): RepoKind => (value === "team" ? "team" : "workspace");
  const one = (entry: CheckpointFields): WorkspaceCheckpoint[] =>
    typeof entry.checkpoint_id === "string" && entry.checkpoint_id.length > 0
      ? [{ id: entry.checkpoint_id, repo: repoOf(entry.checkpoint_repo) }]
      : [];
  if (Array.isArray(body.checkpoints)) {
    return (body.checkpoints as CheckpointFields[]).flatMap(one);
  }
  return one(body);
}

export async function deleteWorkspaceFile(
  agent: string | null,
  path: string,
  scope: WorkspaceScope = "agent",
): Promise<WorkspaceCheckpoint[]> {
  const body = await apiFetch<Parameters<typeof parseWorkspaceCheckpoints>[0]>(
    workspacePath(agent, `/file?path=${encodeURIComponent(path)}`, scope),
    { method: "DELETE" },
  );
  return parseWorkspaceCheckpoints(body);
}

export async function moveWorkspaceFile(
  agent: string | null,
  from: string,
  to: string,
  overwrite = false,
  scope: WorkspaceScope = "agent",
): Promise<void> {
  await apiFetch<unknown>(workspacePath(agent, "/move", scope), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ from, to, overwrite }),
  });
}

function checkpointPath(agent: string | null, repo: RepoKind, sub: string): string {
  const path = `/checkpoints${sub}`;
  return repo === "hub" || repo === "team" ? hubPath(path) : agentPath(requireAgent(agent), path);
}

export async function fetchCheckpoints(
  agent: string | null,
  query: {
    repo: RepoKind;
    path?: string;
    turnId?: string;
    before?: string;
    limit?: number;
  },
): Promise<CheckpointPage> {
  const params = new URLSearchParams({ repo: query.repo });
  if (query.path) params.set("path", query.path);
  if (query.turnId) params.set("turn_id", query.turnId);
  if (query.before) params.set("before", query.before);
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  return apiFetch<CheckpointPage>(checkpointPath(agent, query.repo, `?${params}`));
}

export async function fetchCheckpointStats(
  agent: string | null,
  repo: RepoKind,
): Promise<RepoStats> {
  return apiFetch<RepoStats>(checkpointPath(agent, repo, `/stats?repo=${repo}`));
}

export async function fetchCheckpointDetail(
  agent: string | null,
  id: string,
  repo: RepoKind,
): Promise<CheckpointDetail> {
  return apiFetch<CheckpointDetail>(
    checkpointPath(agent, repo, `/${encodeURIComponent(id)}?repo=${repo}`),
  );
}

export async function fetchCheckpointDiff(
  agent: string | null,
  id: string,
  repo: RepoKind,
  path: string,
): Promise<string | null> {
  const data = await apiFetch<{ diff: string | null }>(
    checkpointPath(
      agent,
      repo,
      `/${encodeURIComponent(id)}/diff?repo=${repo}&path=${encodeURIComponent(path)}`,
    ),
  );
  return data.diff;
}

/** A file's raw text content at a checkpoint. Throws `ApiError` (404 if it's a directory or absent there). */
export async function fetchCheckpointFile(
  agent: string | null,
  id: string,
  repo: RepoKind,
  path: string,
): Promise<string> {
  return apiFetchText(
    checkpointPath(
      agent,
      repo,
      `/${encodeURIComponent(id)}/file?repo=${repo}&path=${encodeURIComponent(path)}`,
    ),
  );
}

export async function restoreCheckpoint(
  agent: string | null,
  id: string,
  repo: RepoKind,
  path: string,
): Promise<RestoreOutcome> {
  return apiFetch<RestoreOutcome>(
    checkpointPath(agent, repo, `/${encodeURIComponent(id)}/restore`),
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ repo, path }),
    },
  );
}

export async function undoCheckpoint(
  agent: string | null,
  id: string,
  repo: RepoKind,
): Promise<UndoOutcome> {
  return apiFetch<UndoOutcome>(checkpointPath(agent, repo, `/${encodeURIComponent(id)}/undo`), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ repo }),
  });
}

async function readCheckpointId(path: string, init?: RequestInit): Promise<string | null> {
  const body = await apiFetch<{ checkpoint_id?: unknown }>(path, init);
  return typeof body.checkpoint_id === "string" && body.checkpoint_id.length > 0
    ? body.checkpoint_id
    : null;
}

export async function fetchCloudStatus(): Promise<CloudStatusResponse> {
  return apiFetch<CloudStatusResponse>(hubPath("/cloud/status"));
}

export async function startCloudSignIn(relayUrl: string, port: string): Promise<string> {
  const { url } = await apiFetch<{ url: string }>(hubPath("/cloud/connect"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ relay_url: relayUrl, port }),
  });
  return url;
}

export async function disconnectCloud(): Promise<void> {
  await apiFetchText(hubPath("/cloud/disconnect"), { method: "POST" });
}

export async function fetchUpdateStatus(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>(hubPath("/update/status"));
}

export async function triggerUpdateCheck(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>(hubPath("/update/check"), { method: "POST" });
}

export async function applyUpdate(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>(hubPath("/update/apply"), { method: "POST" });
}

export async function fetchPushKey(): Promise<string> {
  const data = await apiFetch<PushKeyResponse>(hubPath("/push/key"));
  return data.public_key;
}

export async function fetchPushDevices(): Promise<PushDevice[]> {
  const data = await apiFetch<PushDeviceList>(hubPath("/push/devices"));
  return data.devices;
}

export async function registerPushDevice(
  subscription: WebPushSubscription,
  label: string,
): Promise<PushDevice> {
  const data = await apiFetch<PushDeviceResponse>(hubPath("/push/devices"), {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ subscription, label }),
  });
  return data.device;
}

export async function updatePushDevice(
  id: string,
  change: { label?: string; preferences?: PushPreferencesPatch },
): Promise<PushDevice> {
  const data = await apiFetch<PushDeviceResponse>(
    hubPath(`/push/devices/${encodeURIComponent(id)}`),
    {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(change),
    },
  );
  return data.device;
}

export async function removePushDevice(id: string): Promise<void> {
  await apiFetchText(hubPath(`/push/devices/${encodeURIComponent(id)}`), { method: "DELETE" });
}

export async function sendPushTest(id: string): Promise<PushTestResult> {
  return apiFetch<PushTestResult>(hubPath(`/push/devices/${encodeURIComponent(id)}/test`), {
    method: "POST",
  });
}

export async function fetchAgents(): Promise<AgentListResponse> {
  return apiFetch<AgentListResponse>(hubPath("/agents"));
}

export async function fetchHubStatus(): Promise<HubStatusResponse> {
  return apiFetch<HubStatusResponse>(hubPath("/status"));
}

export async function createAgent(request: CreateAgentRequest): Promise<AgentSummary> {
  return apiFetch<AgentSummary>(hubPath("/agents"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    // The generated type spells absent options as `null`; the wire leaves them out.
    body: JSON.stringify(request, (_key, value: unknown) => value ?? undefined),
  });
}

export async function deleteAgent(name: string): Promise<DeleteOutcome> {
  return apiFetch<DeleteOutcome>(hubPath(`/agents/${encodeURIComponent(name)}`), {
    method: "DELETE",
  });
}

export async function fetchDeletedAgents(): Promise<DeletedAgent[]> {
  const data = await apiFetch<DeletedAgentListResponse>(hubPath("/agents/deleted"));
  return data.agents;
}

/**
 * Restore a deleted agent. Without `checkpointId` the hub uses the last
 * checkpoint taken before the deletion. Throws `ApiError` (404 nothing to
 * restore under that name, 409 the name exists, 400 an unknown checkpoint).
 */
export async function restoreAgent(name: string, checkpointId?: string): Promise<AgentSummary> {
  const request: RestoreAgentRequest = { name, checkpoint_id: checkpointId ?? null };
  return apiFetch<AgentSummary>(hubPath("/agents/restore"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    // The generated type spells an absent checkpoint as `null`; the wire leaves it out.
    body: JSON.stringify(request, (_key, value: unknown) => value ?? undefined),
  });
}

async function agentLifecycle(
  name: string,
  action: "start" | "stop" | "restart",
): Promise<AgentSummary> {
  return apiFetch<AgentSummary>(hubPath(`/agents/${encodeURIComponent(name)}/${action}`), {
    method: "POST",
  });
}

export async function startAgent(name: string): Promise<AgentSummary> {
  return agentLifecycle(name, "start");
}

export async function stopAgent(name: string): Promise<AgentSummary> {
  return agentLifecycle(name, "stop");
}

export async function restartAgent(name: string, force = false): Promise<AgentSummary> {
  if (!force) return agentLifecycle(name, "restart");
  return apiFetch<AgentSummary>(hubPath(`/agents/${encodeURIComponent(name)}/restart?force=true`), {
    method: "POST",
  });
}

export async function setAgentVisibility(
  name: string,
  a2a_visibility: A2aVisibility,
): Promise<AgentSummary> {
  return patchAgent(name, { a2a_visibility });
}

export async function setAgentAutostart(name: string, autostart: boolean): Promise<AgentSummary> {
  return patchAgent(name, { autostart });
}

/** `PATCH /api/hub/agents/{name}`: sends only the fields set, since the hub needs at least one. */
async function patchAgent(name: string, patch: Partial<AgentPatch>): Promise<AgentSummary> {
  return apiFetch<AgentSummary>(hubPath(`/agents/${encodeURIComponent(name)}`), {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(patch),
  });
}

function queryString(params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) search.set(key, String(value));
  }
  const text = search.toString();
  return text === "" ? "" : `?${text}`;
}

export async function fetchOverview(): Promise<OverviewResponse> {
  return apiFetch<OverviewResponse>(hubPath("/overview"));
}

export async function fetchTeamEvents(
  query: { before?: number; after?: number; limit?: number } = {},
): Promise<TeamEventPage> {
  return apiFetch<TeamEventPage>(hubPath(`/events${queryString(query)}`));
}

/** The largest file the hub stages for a message, in bytes. A larger upload is answered `413`. */
export const MAX_UPLOAD_BYTES = 50 * 1024 * 1024;

export const UPLOAD_LIMIT_LABEL = `${String(MAX_UPLOAD_BYTES / (1024 * 1024))} MB`;

export interface UploadedFile {
  id: string;
  filename: string;
  size: number;
}

export interface SendMailRequest {
  to: string[];
  subject: string;
  body: string;
  attachments: string[];
}

/** What `POST /api/hub/mail/threads/{thread_id}/reply` takes. `attachments` are upload ids. */
export interface ReplyToMailRequest {
  body: string;
  attachments: string[];
}

function mailThreadPath(threadId: string, action?: string): string {
  const path = `/mail/threads/${encodeURIComponent(threadId)}`;
  return hubPath(action === undefined ? path : `${path}/${action}`);
}

export async function fetchMailThreads(query: {
  folder: MailFolder;
  agent?: string;
  before?: string;
  limit?: number;
}): Promise<MailThreadPage> {
  const { folder, agent, before, limit } = query;
  return apiFetch<MailThreadPage>(
    hubPath(`/mail/threads${queryString({ folder, agent, before, limit })}`),
  );
}

export async function fetchMailThread(threadId: string): Promise<MailThread> {
  return apiFetch<MailThread>(mailThreadPath(threadId));
}

export async function sendMail(request: SendMailRequest): Promise<MailSendResponse> {
  return apiFetch<MailSendResponse>(hubPath("/mail/send"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function replyToMailThread(
  threadId: string,
  request: ReplyToMailRequest,
): Promise<MailReplyResponse> {
  return apiFetch<MailReplyResponse>(mailThreadPath(threadId, "reply"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
  });
}

export async function markMailThreadRead(threadId: string): Promise<void> {
  await apiFetchText(mailThreadPath(threadId, "read"), { method: "POST" });
}

export async function archiveMailThread(threadId: string): Promise<void> {
  await apiFetchText(mailThreadPath(threadId, "archive"), { method: "POST" });
}

export async function restoreMailThread(threadId: string): Promise<void> {
  await apiFetchText(mailThreadPath(threadId, "restore"), { method: "POST" });
}

/**
 * Stage a file for a message to attach: the body is the file's bytes, so
 * nothing is encoded. Throws `ApiError`; a file over `MAX_UPLOAD_BYTES` is
 * answered `413`. Aborting `signal` cancels the upload, so one the user
 * removed before it finished stages nothing; the call then throws the
 * `AbortError` the fetch rejects with.
 */
export async function uploadFile(
  file: File | Blob,
  filename: string,
  options: { signal?: AbortSignal } = {},
): Promise<UploadedFile> {
  return apiFetch<UploadedFile>(hubPath(`/uploads${queryString({ filename })}`), {
    method: "POST",
    headers: { "Content-Type": file.type || "application/octet-stream" },
    body: file,
    ...(options.signal === undefined ? {} : { signal: options.signal }),
  });
}

export async function fetchTeamsSetupPrereqs(agent: string): Promise<TeamsSetupPrereqs> {
  return apiFetch<TeamsSetupPrereqs>(agentPath(agent, "/teams-setup/prereqs"));
}

export async function fetchTeamsSetupJob(
  agent: string,
  logSince?: number,
): Promise<TeamsSetupJob | null> {
  const query = logSince !== undefined ? `?log_since=${encodeURIComponent(logSince)}` : "";
  try {
    return await apiFetch<TeamsSetupJob>(agentPath(agent, `/teams-setup/job${query}`));
  } catch (err: unknown) {
    if (err instanceof ApiError && err.status === 404) return null;
    throw err;
  }
}

/**
 * Start or resume a Teams setup job.
 * On 409 (already running), returns the currently running job.
 * On 400 (validation failure), throws ApiError with the actionable message.
 */
export async function startTeamsSetupJob(
  agent: string,
  start: TeamsSetupStart,
): Promise<TeamsSetupJob> {
  try {
    return await apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(start),
    });
  } catch (err: unknown) {
    if (err instanceof ApiError && err.status === 409) {
      try {
        return JSON.parse(err.body) as TeamsSetupJob;
      } catch {
        throw err;
      }
    }
    throw err;
  }
}

export async function submitTeamsSetupRedirect(agent: string, url: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/redirect"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url }),
  });
}

export async function cancelTeamsSetupJob(agent: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/cancel"), {
    method: "POST",
  });
}

export async function retryTeamsSetupJob(agent: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/retry"), {
    method: "POST",
  });
}

export async function installTeamsApp(agent: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/install-app"), {
    method: "POST",
  });
}

export function teamsAppPackageUrl(agent: string): string {
  return agentPath(agent, "/teams-setup/job/package");
}

export async function fetchTeamsAppPackage(agent: string): Promise<Blob> {
  const resp = await checkOk(await fetch(agentPath(agent, "/teams-setup/job/package")));
  return resp.blob();
}

export async function deleteTeamsSetupJob(agent: string): Promise<void> {
  await apiFetchText(agentPath(agent, "/teams-setup/job"), { method: "DELETE" });
}

export async function cleanupTeamsSetup(
  agent: string,
  req: CleanupRequest,
): Promise<CleanupResult> {
  return apiFetch<CleanupResult>(agentPath(agent, "/teams-setup/cleanup"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
}
