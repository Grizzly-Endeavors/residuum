// ── Typed fetch wrappers ─────────────────────────────────────────────

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
  HubInboxItem,
  HubInboxPage,
  HubStatusResponse,
  InboxStatus,
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

// ── Cache keys ──────────────────────────────────────────────────────
//
// Exported so other modules (e.g. ws.svelte.ts) can clear them when
// out-of-band signals (gateway reload, WS reconnect) tell us the
// server's view may have changed.

export const CACHE_KEY_TIMEZONE = `GET ${hubPath("/system/timezone")}`;
export const CACHE_KEY_MCP_CATALOG = `GET ${hubPath("/mcp-catalog")}`;
export const CACHE_KEY_HUB_CONFIG_RAW = `GET ${hubPath("/config/raw")}`;

// Agent-scoped entries are keyed by agent, so a fetch for one agent can't
// fill or clear another agent's entry.
function agentCacheKey(agent: string, sub: string): string {
  return `GET ${agentBase(agent)}${sub}`;
}

export const cacheKeyStatus = (agent: string): string => agentCacheKey(agent, "/status");
export const cacheKeyConfigRaw = (agent: string): string => agentCacheKey(agent, "/config/raw");
export const cacheKeyProvidersRaw = (agent: string): string =>
  agentCacheKey(agent, "/providers/raw");
export const cacheKeyMcpRaw = (agent: string): string => agentCacheKey(agent, "/mcp/raw");

// ── Error class + fetch helpers ─────────────────────────────────────

/** Structured error from a failed API response. */
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

/**
 * The validation result a raw-file PUT reports through a 400. Those
 * endpoints answer invalid input with `400 {valid: false, error}`, and
 * `apiFetch` throws on any non-2xx, so callers would otherwise only ever see
 * a generic failure instead of the validation message.
 */
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

/** A workspace write conflict: the file changed since it was last read. */
export interface WorkspaceWriteConflict {
  error: string;
  /** The file's version now, or `null` if it no longer exists. */
  currentVersion: string | null;
}

/**
 * The conflict a workspace file `PUT` reports through a `412` when its
 * `If-Match` no longer matches the file's current version — someone else
 * (the agent, another tab) saved it first. `apiFetch`/`apiFetchText` throw
 * on any non-2xx, so callers would otherwise only see a generic failure
 * instead of the version needed to reload or force an overwrite.
 */
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

/**
 * `PATCH` a JSON diff (see `lib/settings-toml.ts`'s diff builders) and
 * report the same `{valid, error}` shape a raw-file PUT would. Skips the
 * request entirely when `diff` has no keys — an empty patch is a no-op, not
 * a network call.
 */
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
    throw new ApiError(resp.status, resp.statusText, body);
  }
  return resp;
}

/** Fetch wrapper that throws `ApiError` on non-ok responses. */
async function apiFetch<T>(input: RequestInfo | URL, init?: RequestInit): Promise<T> {
  const resp = await checkOk(await fetch(input, init));
  return (await resp.json()) as T;
}

/** Fetch wrapper for plain text responses that throws `ApiError` on non-ok. */
async function apiFetchText(input: RequestInfo | URL, init?: RequestInit): Promise<string> {
  const resp = await checkOk(await fetch(input, init));
  return resp.text();
}

// ── Core API wrappers ───────────────────────────────────────────────

export async function fetchStatus(agent: string): Promise<StatusResponse> {
  return cachedFetch(cacheKeyStatus(agent), () =>
    apiFetch<StatusResponse>(agentPath(agent, "/status")),
  );
}

/**
 * Throws `ApiError` when the server fails to serve the recent segment — the
 * caller is responsible for surfacing the failure to the user. Swallowing
 * silently would hide real corruption (e.g. a malformed `recent_messages.json`)
 * and make the chat feed look empty when it isn't.
 */
export async function fetchChatHistory(agent: string): Promise<RecentHistorySegment> {
  const segment = await apiFetch<ChatHistorySegment>(agentPath(agent, "/chat/history"));
  if (segment.kind === "recent") return segment;
  throw new Error(
    `unexpected chat history kind "${segment.kind}" — server must return Recent for the base call`,
  );
}

/**
 * Fetch the main agent's cumulative token usage totals, for the chat
 * footer to render correctly on connect/reconnect before the next model
 * call. Not cached — the WebSocket carries live updates from here on, this
 * is only the connect-time seed. Callers should degrade quietly on
 * failure (the footer simply starts blank) rather than surfacing an error
 * for this quiet, non-critical feature.
 */
export async function fetchUsageTotals(agent: string): Promise<SessionUsageTotals> {
  return apiFetch<SessionUsageTotals>(agentPath(agent, "/usage"));
}

/**
 * Fetch an episode segment by cursor. Episodes are immutable, so the result
 * is cached for the life of the browser session (and across reloads).
 *
 * Throws `ApiError` on failure — callers must decide how to surface the
 * error. A 404 (episode not found) is surfaced the same as any other
 * failure; the caller can inspect `ApiError.status` if it needs to branch.
 */
export async function fetchChatSegment(
  agent: string,
  episodeId: string,
): Promise<EpisodeHistorySegment> {
  // Built inline rather than as a CACHE_KEY_* constant because it's per-episode.
  // The key string must match the url string exactly — if you change one, change
  // the other, or cache lookups will miss.
  const url = agentPath(agent, `/chat/history?episode=${encodeURIComponent(episodeId)}`);
  const segment = await cachedFetch(`GET ${url}`, () => apiFetch<ChatHistorySegment>(url));
  if (segment.kind === "episode") return segment;
  throw new Error(
    `unexpected chat history kind "${segment.kind}" — server must return Episode for ?episode=`,
  );
}

// ── Feedback / bug-report API wrappers ──────────────────────────────

/** Receipt returned by the developer ingest service after submission. */
export interface FeedbackReceipt {
  public_id: string;
  submitted_at: string;
}

/** Severity values accepted by the bug-report endpoint. */
export type BugSeverity = "broken" | "wrong" | "annoying";

/** POST a structured bug report through the gateway to the developer endpoint. */
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

/** POST a free-form feedback message through the gateway. */
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

// ── Setup API wrappers ──────────────────────────────────────────────

/** Graceful fallback: uses browser timezone when server is unreachable during setup. */
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

  // Onboarding lists a provider's models before any agent exists.
  const path =
    agent === null ? hubPath("/providers/models") : agentPath(agent, "/providers/models");
  return apiFetch<ModelsResponse>(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** The decision model form's values, saved or not: what the model list and the connection test use. */
export interface SystemOneForm {
  provider: string;
  url?: string;
  model?: string;
  /** A literal key or a `secret:<name>` reference. */
  api_key?: string;
  keep_alive?: string;
}

/** `POST /api/hub/system-one/models`. */
export interface SystemOneModelsResponse {
  models: { id: string; description: string | null }[];
  /** Why the list couldn't be read, in plain words. */
  error?: string;
}

/** `POST /api/hub/system-one/test`. */
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

/** Ask the decision model the form describes one small question. Throws `ApiError` only when the hub can't be reached. */
export async function testSystemOne(form: SystemOneForm): Promise<SystemOneTestResponse> {
  return apiFetch<SystemOneTestResponse>(hubPath("/system-one/test"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(form),
  });
}

/** The MCP catalog. Throws `ApiError`; a failed read isn't cached, so the caller can offer Try again. */
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

/** Everything onboarding writes: the hub config, and the first agent's name and files. */
export interface CompleteSetupPayload {
  hubConfig: string;
  agentName: string;
  /** The user's name, written to the team's USER.md. Empty when they skipped it. */
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

// ── Settings API wrappers ────────────────────────────────────────────

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

/** Merge a diff (from `diffConfigFields`) into `config.toml` on the server. */
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

/** The hub's `config.toml` (timezone, gateway, cloud, A2A listener, tracing, session limits). */
export async function fetchHubConfigRaw(): Promise<string> {
  return cachedFetch(CACHE_KEY_HUB_CONFIG_RAW, () => apiFetchText(hubPath("/config/raw")));
}

export async function putHubConfigRaw(toml: string): Promise<ValidateResponse> {
  return putValidated(hubPath("/config/raw"), "text/plain", toml, CACHE_KEY_HUB_CONFIG_RAW);
}

/** Merge a diff (the hub half of `splitConfigPatch`) into the hub's `config.toml`. */
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

/** Merge a diff (from `diffProviders`/`modelRoleJson`) into `providers.toml` on the server. */
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

/** Problems in `json` as the agent's `mcp.json`, without writing it. Throws when the check can't be made. */
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

/** Merge a diff (from `diffMcpServers`) into `mcp.json` on the server. */
export async function patchMcp(
  agent: string,
  diff: Record<string, unknown>,
): Promise<ValidateResponse> {
  return patchValidated(agentPath(agent, "/mcp/patch"), diff, cacheKeyMcpRaw(agent));
}

/** The names of the stored secrets (never their values). Throws `ApiError` on failure. */
export async function fetchSecretNames(): Promise<string[]> {
  const data = await apiFetch<SecretsListResponse>(hubPath("/secrets"));
  return data.names;
}

/** Graceful fallback: returns empty on failure (secrets list is non-critical). */
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

// ── Agent keys API wrappers ─────────────────────────────────────────

/** Throws `ApiError` on failure; the caller surfaces it. */
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

// ── A2A API wrappers ──────────────────────────────────────────────────

/** Live A2A status. Throws `ApiError` on failure; the caller surfaces it. */
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

/** Throws `ApiError` on failure; the caller surfaces it. */
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

/**
 * Remote agents from `config/a2a.json` plus any discovered siblings.
 * Throws `ApiError` on failure; the caller surfaces it.
 */
export async function fetchA2aAgents(agent: string): Promise<A2aRemoteAgent[]> {
  return apiFetch<A2aRemoteAgent[]>(agentPath(agent, "/a2a/agents"));
}

/** Open tasks the agent sent to remote agents, newest first. Throws `ApiError`. */
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

/** Stop watching a task without reaching its agent. Throws `ApiError` (`404` when it already ended). */
export async function stopWatchingOutboundA2aTask(
  agent: string,
  taskId: string,
): Promise<OutboundA2aTaskSummary> {
  return apiFetch<OutboundA2aTaskSummary>(
    agentPath(agent, `/a2a/outbound/${encodeURIComponent(taskId)}/stop-watching`),
    { method: "POST" },
  );
}

/**
 * `config/a2a.json` as it is now. Never cached: the agent edits it too, and
 * an editor opened on an old copy would save over the agent's change.
 */
export async function fetchA2aAgentsRaw(agent: string): Promise<string> {
  return apiFetchText(agentPath(agent, "/a2a/agents/raw"));
}

/**
 * Save `config/a2a.json`. Always saves, even when invalid — the loader
 * skips an unusable agent entry with a warning and keeps every other agent
 * running, so the response reports a diagnostic instead of the write being
 * rejected. Same shape as `putConfigRaw`/`putProvidersRaw`/`putMcpRaw`.
 */
export async function putA2aAgentsRaw(agent: string, content: string): Promise<ValidateResponse> {
  return putValidated(agentPath(agent, "/a2a/agents/raw"), "application/json", content, null);
}

// ── Agent sessions API wrappers ─────────────────────────────────────

/**
 * List live sessions plus one page of completed runs, newest first, both
 * limited to `category` when given. Never cached: the listing changes
 * whenever a session starts or finishes.
 *
 * Throws `ApiError` on failure; the caller surfaces it.
 */
export async function fetchSessions(
  agent: string,
  query: {
    category?: SessionCategory;
    before?: string;
    limit?: number;
    address?: string;
    /** Only sessions that workbench artifact started, not sessions those spawned in turn. */
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

/**
 * Fetch one run's transcript, live or completed. Not cached: a live run's
 * transcript grows as it works.
 *
 * Throws `ApiError` on failure (404 for an unknown run).
 */
export async function fetchSessionTranscript(
  agent: string,
  runId: string,
): Promise<SessionTranscriptResponse> {
  return apiFetch<SessionTranscriptResponse>(
    agentPath(agent, `/sessions/runs/${encodeURIComponent(runId)}/transcript`),
  );
}

/**
 * Send the session at `address` a message as the owner: delivered to a live
 * run, or starting a new run of a finished one. Resolves to where it landed.
 *
 * Throws `ApiError` with `{ error, code }`: `404` for an unknown address,
 * `409` when the session is busy, `502` when delivery failed.
 */
export async function messageSession(
  agent: string,
  address: string,
  content: string,
): Promise<SessionDeliveryOutcome> {
  const reply = await apiFetch<{ outcome: SessionDeliveryOutcome }>(
    agentPath(agent, `/sessions/${encodeURIComponent(address)}/messages`),
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ content }),
    },
  );
  return reply.outcome;
}

/**
 * Stop the live session at `address`. The run's `completing` and
 * `session_completed` frames follow. Throws `ApiError` (`404` when nothing
 * there can still be stopped).
 */
export async function stopSession(agent: string, address: string): Promise<void> {
  await apiFetch<unknown>(agentPath(agent, `/sessions/${encodeURIComponent(address)}/stop`), {
    method: "POST",
  });
}

// ── Scheduled view API wrappers ──────────────────────────────────────

/** Every pulse in HEARTBEAT.yml. Not cached: run state changes live. */
export async function fetchScheduledPulses(agent: string): Promise<PulseInfo[]> {
  return apiFetch<PulseInfo[]>(agentPath(agent, "/scheduled/pulses"));
}

/** Flip a pulse's `enabled` field in HEARTBEAT.yml. Throws `ApiError` (404 if the pulse is gone). */
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

/** Every pending scheduled action. Not cached: it changes as actions fire. */
export async function fetchScheduledActions(agent: string): Promise<ActionInfo[]> {
  return apiFetch<ActionInfo[]>(agentPath(agent, "/scheduled/actions"));
}

/** Cancel a pending scheduled action. Throws `ApiError` (404 if already gone). */
export async function cancelScheduledAction(agent: string, id: string): Promise<void> {
  await apiFetch<unknown>(agentPath(agent, `/scheduled/actions/${encodeURIComponent(id)}`), {
    method: "DELETE",
  });
}

// ── Workbench API wrappers ──────────────────────────────────────────

/** Every workbench artifact, most recently modified first. Not cached: artifacts change live. */
export async function fetchWorkbenchArtifacts(): Promise<ArtifactSummary[]> {
  return apiFetch<ArtifactSummary[]>(teamPath("/workbench/artifacts"));
}

/** Where artifacts are served, locally and through the relay. Not cached: the relay connection changes. */
export async function fetchWorkbenchInfo(): Promise<WorkbenchInfo> {
  return apiFetch<WorkbenchInfo>(teamPath("/workbench/info"));
}

/** What deleting an artifact removed, and the team checkpoint that can bring it back. */
export interface ArtifactDeletion {
  /** Team-relative paths of everything removed (`workbench/chart.html`, `workbench/graph`). */
  paths: string[];
  /** The pre-delete team checkpoint, or `null` when none was recorded. */
  checkpointId: string | null;
}

/** Delete an artifact and its data files. Throws `ApiError` (404 if already gone). */
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

// ── Workspace API wrappers ──────────────────────────────────────────

/**
 * A workspace API path. Scope `agent` addresses `agent`'s tree (where the
 * shared tree shows up under `team/`); `team` addresses the shared tree
 * directly, with paths relative to `team/`. Only the agent scope needs an
 * agent: `null` is for the team scope, and throws `NoAgentSelectedError` for
 * the agent one.
 */
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

/** A workspace file's content plus the version to send back as `If-Match`. */
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

/** Diagnostics for `content` as if it were saved to `path`, without writing
 * anything. Empty means either the content is clean or `path` isn't one of
 * the strictly-parsed files the server checks. Graceful fallback: a network
 * or server error returns no diagnostics rather than interrupting typing. */
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

/** A pre-action checkpoint and the repository that holds it. */
export interface WorkspaceCheckpoint {
  id: string;
  repo: RepoKind;
}

interface CheckpointFields {
  checkpoint_id?: unknown;
  checkpoint_repo?: unknown;
}

/**
 * The checkpoints a workspace-API response names: `checkpoint_id` +
 * `checkpoint_repo` for one, or a `checkpoints` list when the action spanned
 * the agent and team directories. Empty when none was recorded. A response
 * without `checkpoint_repo` came from an agent-only path, so the workspace
 * repository holds it.
 */
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

/** Delete a workspace file or `team/...` file. Throws `ApiError` (404 if already gone).
 * Returns the pre-delete checkpoint(s), empty when none was recorded. */
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

/** Move or rename a workspace file. Throws `ApiError` (409 if `to` exists and `overwrite` isn't set). */
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

// ── Checkpoints API wrappers ─────────────────────────────────────────

/**
 * Hub and team repositories live under the hub; workspace and agent-config
 * under the agent. Only those two need an agent: `null` is for the hub-level
 * repositories, and throws `NoAgentSelectedError` for an agent-level one.
 */
function checkpointPath(agent: string | null, repo: RepoKind, sub: string): string {
  const path = `/checkpoints${sub}`;
  return repo === "hub" || repo === "team" ? hubPath(path) : agentPath(requireAgent(agent), path);
}

/**
 * One page of checkpoints, newest first. `path` restricts to checkpoints
 * that changed it; `turnId` restricts to a single turn's checkpoints (its
 * turn-start/turn-end pair); `before`/`limit` page. Never cached — the
 * list changes on every turn and action.
 */
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

/** On-disk size, checkpoint count, and oldest checkpoint for a repository. */
export async function fetchCheckpointStats(
  agent: string | null,
  repo: RepoKind,
): Promise<RepoStats> {
  return apiFetch<RepoStats>(checkpointPath(agent, repo, `/stats?repo=${repo}`));
}

/** A checkpoint's metadata plus the paths it changed. */
export async function fetchCheckpointDetail(
  agent: string | null,
  id: string,
  repo: RepoKind,
): Promise<CheckpointDetail> {
  return apiFetch<CheckpointDetail>(
    checkpointPath(agent, repo, `/${encodeURIComponent(id)}?repo=${repo}`),
  );
}

/** Unified diff for one file at a checkpoint, `null` if it didn't change there. */
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

/** Restore `path` to its content at checkpoint `id`. Checkpoints the result, so it can itself be undone. */
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

/** Undo everything checkpoint `id` changed, skipping any path changed again since. */
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

/** `checkpoint_id` from a delete/revoke response, or `null` when the server
 * recorded none (the checkpoint failed, or the field is missing). */
async function readCheckpointId(path: string, init?: RequestInit): Promise<string | null> {
  const body = await apiFetch<{ checkpoint_id?: unknown }>(path, init);
  return typeof body.checkpoint_id === "string" && body.checkpoint_id.length > 0
    ? body.checkpoint_id
    : null;
}

// ── Cloud API wrappers ──────────────────────────────────────────────

export async function fetchCloudStatus(): Promise<CloudStatusResponse> {
  return apiFetch<CloudStatusResponse>(hubPath("/cloud/status"));
}

export async function disconnectCloud(): Promise<void> {
  await apiFetchText(hubPath("/cloud/disconnect"), { method: "POST" });
}

// ── Update API wrappers ──────────────────────────────────────────────

export async function fetchUpdateStatus(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>(hubPath("/update/status"));
}

export async function triggerUpdateCheck(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>(hubPath("/update/check"), { method: "POST" });
}

export async function applyUpdate(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>(hubPath("/update/apply"), { method: "POST" });
}

// ── Web Push API wrappers ────────────────────────────────────────────
//
// Every one throws `ApiError` on failure; the caller surfaces it.

/** The hub's VAPID public key, base64url: what a browser subscribes with. */
export async function fetchPushKey(): Promise<string> {
  const data = await apiFetch<PushKeyResponse>(hubPath("/push/key"));
  return data.public_key;
}

/** Every device that receives notifications, oldest first. */
export async function fetchPushDevices(): Promise<PushDevice[]> {
  const data = await apiFetch<PushDeviceList>(hubPath("/push/devices"));
  return data.devices;
}

/** Register a browser's subscription, or update the device already registered for it. */
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

/** Rename a device or change some of its preferences. */
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

/** Stop sending to a device. */
export async function removePushDevice(id: string): Promise<void> {
  await apiFetchText(hubPath(`/push/devices/${encodeURIComponent(id)}`), { method: "DELETE" });
}

/** Send the test notification to a device and say whether its push service took it. */
export async function sendPushTest(id: string): Promise<PushTestResult> {
  return apiFetch<PushTestResult>(hubPath(`/push/devices/${encodeURIComponent(id)}/test`), {
    method: "POST",
  });
}

// ── Hub lifecycle API wrappers ───────────────────────────────────────

/** Every agent with its state, sorted by name, with the activity and stopping set beside the list. */
export async function fetchAgents(): Promise<AgentListResponse> {
  return apiFetch<AgentListResponse>(hubPath("/agents"));
}

/** Hub status: version, uptime, tunnel, and agent counts by state. */
export async function fetchHubStatus(): Promise<HubStatusResponse> {
  return apiFetch<HubStatusResponse>(hubPath("/status"));
}

/** Create an agent. Throws `ApiError` (400 invalid name, 409 name exists). */
export async function createAgent(request: CreateAgentRequest): Promise<AgentSummary> {
  return apiFetch<AgentSummary>(hubPath("/agents"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    // The generated type spells absent options as `null`; the wire leaves them out.
    body: JSON.stringify(request, (_key, value: unknown) => value ?? undefined),
  });
}

/** Delete an agent. `checkpoint_id` names the checkpoint it can be restored from. */
export async function deleteAgent(name: string): Promise<DeleteOutcome> {
  return apiFetch<DeleteOutcome>(hubPath(`/agents/${encodeURIComponent(name)}`), {
    method: "DELETE",
  });
}

/** Deleted agents that can be restored, newest deletion first. */
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

export async function restartAgent(name: string): Promise<AgentSummary> {
  return agentLifecycle(name, "restart");
}

/** Make an agent's A2A card public or private. */
export async function setAgentVisibility(
  name: string,
  a2a_visibility: A2aVisibility,
): Promise<AgentSummary> {
  return patchAgent(name, { a2a_visibility });
}

/** Turn an agent's autostart on or off. */
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

// ── Team overview, team events and the cross-agent inbox ────────────

/** `?a=1&b=2` from the parameters that are set, or `""` when none are. */
function queryString(params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) search.set(key, String(value));
  }
  const text = search.toString();
  return text === "" ? "" : `?${text}`;
}

/** Every agent's overview, sorted by name, with the boot id of the hub process that answered. Throws `ApiError`. */
export async function fetchOverview(): Promise<OverviewResponse> {
  return apiFetch<OverviewResponse>(hubPath("/overview"));
}

/**
 * One page of the team event log, newest first: entries older than `before`
 * and newer than `after`, at most `limit` (50 by default, 200 at most).
 * Throws `ApiError`.
 */
export async function fetchTeamEvents(
  query: { before?: number; after?: number; limit?: number } = {},
): Promise<TeamEventPage> {
  return apiFetch<TeamEventPage>(hubPath(`/events${queryString(query)}`));
}

/**
 * One page of every agent's user inbox, newest first: the active items by
 * default, one agent's with `agent`, the page after a `next_cursor` with
 * `before`. Throws `ApiError`.
 */
export async function fetchHubInbox(
  query: { status?: InboxStatus; agent?: string; before?: string; limit?: number } = {},
): Promise<HubInboxPage> {
  return apiFetch<HubInboxPage>(hubPath(`/inbox${queryString(query)}`));
}

/** The hub's route for one item of `agent`'s user inbox, followed by `action`. */
function hubInboxItemPath(agent: string, id: string, action: string): string {
  return hubPath(`/inbox/${encodeURIComponent(agent)}/${encodeURIComponent(id)}/${action}`);
}

/**
 * Mark an item read, whether it is in the inbox or the archive, and get it
 * back. Throws `ApiError`.
 */
export async function markHubInboxItemRead(agent: string, id: string): Promise<HubInboxItem> {
  const { item } = await apiFetch<{ item: HubInboxItem }>(hubInboxItemPath(agent, id, "read"), {
    method: "PUT",
  });
  return item;
}

/** Move an item from the inbox to the archive. Throws `ApiError`. */
export async function archiveHubInboxItem(agent: string, id: string): Promise<HubInboxItem> {
  const { item } = await apiFetch<{ item: HubInboxItem }>(hubInboxItemPath(agent, id, "archive"), {
    method: "POST",
  });
  return item;
}

/** Move an archived item back to the inbox. Throws `ApiError`. */
export async function restoreHubInboxItem(agent: string, id: string): Promise<HubInboxItem> {
  const { item } = await apiFetch<{ item: HubInboxItem }>(hubInboxItemPath(agent, id, "restore"), {
    method: "POST",
  });
  return item;
}

// ── Teams setup API ─────────────────────────────────────────────────

/** Fetch prerequisite check results for Teams setup. */
export async function fetchTeamsSetupPrereqs(agent: string): Promise<TeamsSetupPrereqs> {
  return apiFetch<TeamsSetupPrereqs>(agentPath(agent, "/teams-setup/prereqs"));
}

/**
 * Fetch current Teams setup job state. Returns `null` on 404 when no job exists.
 * When `logSince` is given, only log lines with `seq > logSince` are returned.
 */
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

/** Submit a redirect URL from browser authentication. */
export async function submitTeamsSetupRedirect(agent: string, url: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/redirect"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url }),
  });
}

/** Cancel a running Teams setup job and kill its process tree. */
export async function cancelTeamsSetupJob(agent: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/cancel"), {
    method: "POST",
  });
}

/** Retry a failed or cancelled Teams setup job from the failed phase. */
export async function retryTeamsSetupJob(agent: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/retry"), {
    method: "POST",
  });
}

/** Install the generated Teams app package into Teams via ATK. */
export async function installTeamsApp(agent: string): Promise<TeamsSetupJob> {
  return apiFetch<TeamsSetupJob>(agentPath(agent, "/teams-setup/job/install-app"), {
    method: "POST",
  });
}

/** Direct URL for downloading the app package zip. */
export function teamsAppPackageUrl(agent: string): string {
  return agentPath(agent, "/teams-setup/job/package");
}

/** Fetch the app package zip as a Blob. */
export async function fetchTeamsAppPackage(agent: string): Promise<Blob> {
  const resp = await checkOk(await fetch(agentPath(agent, "/teams-setup/job/package")));
  return resp.blob();
}

/** Forget a finished Teams setup job. Throws 409 if still running. */
export async function deleteTeamsSetupJob(agent: string): Promise<void> {
  await apiFetchText(agentPath(agent, "/teams-setup/job"), { method: "DELETE" });
}

/** Clean up local project files, CLI install, or M365 session. */
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
