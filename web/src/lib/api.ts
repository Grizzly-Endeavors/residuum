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
  A2aAgentsRawResponse,
  WorkspaceEntry,
  WorkspaceWriteResponse,
  WorkspaceValidateResponse,
  Diagnostic,
  CloudStatusResponse,
  UpdateStatusResponse,
  SessionCategory,
  SessionListResponse,
  SessionTranscriptResponse,
  SessionUsageTotals,
  WorkbenchInfo,
  ArtifactSummary,
} from "./types";
import { cachedFetch, invalidate } from "./cache";

// ── Cache keys ──────────────────────────────────────────────────────
//
// Exported so other modules (e.g. ws.svelte.ts) can clear them when
// out-of-band signals (gateway reload, WS reconnect) tell us the
// server's view may have changed.

export const CACHE_KEY_STATUS = "GET /api/status";
export const CACHE_KEY_TIMEZONE = "GET /api/system/timezone";
export const CACHE_KEY_MCP_CATALOG = "GET /api/mcp-catalog";
export const CACHE_KEY_CONFIG_RAW = "GET /api/config/raw";
export const CACHE_KEY_PROVIDERS_RAW = "GET /api/providers/raw";
export const CACHE_KEY_MCP_RAW = "GET /api/mcp/raw";
export const CACHE_KEY_A2A_AGENTS_RAW = "GET /api/a2a/agents/raw";

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

async function putValidated(
  path: string,
  contentType: string,
  body: string,
  cacheKey: string,
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
    invalidate(cacheKey);
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

export async function fetchStatus(): Promise<StatusResponse> {
  return cachedFetch(CACHE_KEY_STATUS, () => apiFetch<StatusResponse>("/api/status"));
}

/**
 * Throws `ApiError` when the server fails to serve the recent segment — the
 * caller is responsible for surfacing the failure to the user. Swallowing
 * silently would hide real corruption (e.g. a malformed `recent_messages.json`)
 * and make the chat feed look empty when it isn't.
 */
export async function fetchChatHistory(): Promise<RecentHistorySegment> {
  const segment = await apiFetch<ChatHistorySegment>("/api/chat/history");
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
export async function fetchUsageTotals(): Promise<SessionUsageTotals> {
  return apiFetch<SessionUsageTotals>("/api/usage");
}

/**
 * Fetch an episode segment by cursor. Episodes are immutable, so the result
 * is cached for the life of the browser session (and across reloads).
 *
 * Throws `ApiError` on failure — callers must decide how to surface the
 * error. A 404 (episode not found) is surfaced the same as any other
 * failure; the caller can inspect `ApiError.status` if it needs to branch.
 */
export async function fetchChatSegment(episodeId: string): Promise<EpisodeHistorySegment> {
  // Built inline rather than as a CACHE_KEY_* constant because it's per-episode.
  // The key string must match the url string exactly — if you change one, change
  // the other, or cache lookups will miss.
  const url = `/api/chat/history?episode=${encodeURIComponent(episodeId)}`;
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
  return apiFetch<FeedbackReceipt>("/api/tracing/bug-report", {
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
  return apiFetch<FeedbackReceipt>("/api/tracing/feedback", {
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
      apiFetch<TimezoneResponse>("/api/system/timezone"),
    );
    return data.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  }
}

export async function fetchProviderModels(
  provider: string,
  apiKey?: string,
  url?: string,
): Promise<ModelsResponse> {
  const body: Record<string, string> = { provider };
  if (apiKey) body.api_key = apiKey;
  if (url) body.url = url;

  return apiFetch<ModelsResponse>("/api/providers/models", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** Graceful fallback: catalog is optional — returns empty on failure. */
export async function fetchMcpCatalog(): Promise<McpCatalogEntry[]> {
  try {
    return await cachedFetch(CACHE_KEY_MCP_CATALOG, () =>
      apiFetch<McpCatalogEntry[]>("/api/mcp-catalog"),
    );
  } catch {
    return [];
  }
}

export async function storeSecret(name: string, value: string): Promise<SecretResponse> {
  return apiFetch<SecretResponse>("/api/secrets", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, value }),
  });
}

export async function completeSetup(
  config: string,
  providers: string,
  mcpJson?: string,
): Promise<ValidateResponse> {
  const payload: Record<string, string> = { config, providers };
  if (mcpJson) payload.mcp_json = mcpJson;
  return apiFetch<ValidateResponse>("/api/config/complete-setup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
}

// ── Settings API wrappers ────────────────────────────────────────────

export async function fetchConfigRaw(): Promise<string> {
  return cachedFetch(CACHE_KEY_CONFIG_RAW, () => apiFetchText("/api/config/raw"));
}

export async function putConfigRaw(toml: string): Promise<ValidateResponse> {
  return putValidated("/api/config/raw", "text/plain", toml, CACHE_KEY_CONFIG_RAW);
}

/** Merge a diff (from `diffConfigFields`) into `config.toml` on the server. */
export async function patchConfig(diff: Record<string, unknown>): Promise<ValidateResponse> {
  return patchValidated("/api/config/patch", diff, CACHE_KEY_CONFIG_RAW);
}

export async function validateConfig(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/config/validate", {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchProvidersRaw(): Promise<string> {
  return cachedFetch(CACHE_KEY_PROVIDERS_RAW, () => apiFetchText("/api/providers/raw"));
}

export async function putProvidersRaw(toml: string): Promise<ValidateResponse> {
  return putValidated("/api/providers/raw", "text/plain", toml, CACHE_KEY_PROVIDERS_RAW);
}

/** Merge a diff (from `diffProviders`/`modelRoleJson`) into `providers.toml` on the server. */
export async function patchProviders(diff: Record<string, unknown>): Promise<ValidateResponse> {
  return patchValidated("/api/providers/patch", diff, CACHE_KEY_PROVIDERS_RAW);
}

export async function validateProviders(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/providers/validate", {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchMcpRaw(): Promise<string> {
  return cachedFetch(CACHE_KEY_MCP_RAW, () => apiFetchText("/api/mcp/raw"));
}

export async function putMcpRaw(json: string): Promise<ValidateResponse> {
  return putValidated("/api/mcp/raw", "application/json", json, CACHE_KEY_MCP_RAW);
}

/** Merge a diff (from `diffMcpServers`) into `mcp.json` on the server. */
export async function patchMcp(diff: Record<string, unknown>): Promise<ValidateResponse> {
  return patchValidated("/api/mcp/patch", diff, CACHE_KEY_MCP_RAW);
}

/** Graceful fallback: returns empty on failure (secrets list is non-critical). */
export async function listSecrets(): Promise<string[]> {
  try {
    const data = await apiFetch<SecretsListResponse>("/api/secrets");
    return data.names;
  } catch {
    return [];
  }
}

export async function deleteSecret(name: string): Promise<void> {
  await apiFetchText(`/api/secrets/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

// ── Agent keys API wrappers ─────────────────────────────────────────

/** Throws `ApiError` on failure; the caller surfaces it. */
export async function fetchAgentKeys(): Promise<AgentKeyInfo[]> {
  const data = await apiFetch<AgentKeysListResponse>("/api/agent-keys");
  return data.keys;
}

export async function storeAgentKey(
  name: string,
  value: string,
  description: string,
): Promise<SetAgentKeyResponse> {
  return apiFetch<SetAgentKeyResponse>("/api/agent-keys", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, value, description }),
  });
}

export async function deleteAgentKey(name: string): Promise<void> {
  await apiFetchText(`/api/agent-keys/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

// ── A2A API wrappers ──────────────────────────────────────────────────

/** Live A2A status. Throws `ApiError` on failure; the caller surfaces it. */
export async function fetchA2aStatus(): Promise<A2aStatusResponse> {
  return apiFetch<A2aStatusResponse>("/api/a2a/status");
}

/**
 * The Agent Card as currently served. Throws `ApiError` — including a `503`
 * (`status` on the error) when the workspace agent card file is invalid,
 * whose body is the plain-language reason.
 */
export async function fetchA2aCard(): Promise<A2aAgentCard> {
  return apiFetch<A2aAgentCard>("/api/a2a/card");
}

/** Throws `ApiError` on failure; the caller surfaces it. */
export async function fetchA2aKeys(): Promise<A2aKeyInfo[]> {
  const data = await apiFetch<A2aKeysListResponse>("/api/a2a/keys");
  return data.keys;
}

export async function createA2aKey(
  name: string,
  description: string,
): Promise<CreateA2aKeyResponse> {
  return apiFetch<CreateA2aKeyResponse>("/api/a2a/keys", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, description: description || undefined }),
  });
}

export async function revokeA2aKey(name: string): Promise<void> {
  await apiFetchText(`/api/a2a/keys/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

/**
 * Remote agents from `config/a2a.json` plus any discovered siblings.
 * Throws `ApiError` on failure; the caller surfaces it.
 */
export async function fetchA2aAgents(): Promise<A2aRemoteAgent[]> {
  return apiFetch<A2aRemoteAgent[]>("/api/a2a/agents");
}

export async function fetchA2aAgentsRaw(): Promise<string> {
  return cachedFetch(CACHE_KEY_A2A_AGENTS_RAW, async () => {
    const data = await apiFetch<A2aAgentsRawResponse>("/api/a2a/agents/raw");
    return data.content;
  });
}

/**
 * Save `config/a2a.json`. Unlike the config/providers/mcp raw editors, a
 * validation failure (`400`) is reported as `{ valid: false, error }` rather
 * than thrown, so the editor can show the reason inline. Any other failure
 * (network, `5xx`) still throws `ApiError` for the caller to surface.
 */
export async function putA2aAgentsRaw(content: string): Promise<ValidateResponse> {
  try {
    await apiFetchText("/api/a2a/agents/raw", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ content }),
    });
    return { valid: true };
  } catch (err: unknown) {
    const validation = validationFromApiError(err);
    if (validation) return validation;
    throw err;
  } finally {
    invalidate(CACHE_KEY_A2A_AGENTS_RAW);
  }
}

// ── Agent sessions API wrappers ─────────────────────────────────────

/**
 * List live sessions plus one page of completed runs, newest first, both
 * limited to `category` when given. Never cached: the listing changes
 * whenever a session starts or finishes.
 *
 * Throws `ApiError` on failure; the caller surfaces it.
 */
export async function fetchSessions(query: {
  category?: SessionCategory;
  before?: string;
  limit?: number;
  address?: string;
  /** Only sessions that workbench artifact started, not sessions those spawned in turn. */
  artifact?: string;
}): Promise<SessionListResponse> {
  const params = new URLSearchParams();
  if (query.category) params.set("category", query.category);
  if (query.before) params.set("before", query.before);
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  if (query.address) params.set("address", query.address);
  if (query.artifact) params.set("artifact", query.artifact);
  const qs = params.toString();
  return apiFetch<SessionListResponse>(`/api/sessions${qs ? `?${qs}` : ""}`);
}

/**
 * Fetch one run's transcript, live or completed. Not cached: a live run's
 * transcript grows as it works.
 *
 * Throws `ApiError` on failure (404 for an unknown run).
 */
export async function fetchSessionTranscript(runId: string): Promise<SessionTranscriptResponse> {
  return apiFetch<SessionTranscriptResponse>(
    `/api/sessions/runs/${encodeURIComponent(runId)}/transcript`,
  );
}

// ── Workbench API wrappers ──────────────────────────────────────────

/** Every workbench artifact, most recently modified first. Not cached: artifacts change live. */
export async function fetchWorkbenchArtifacts(): Promise<ArtifactSummary[]> {
  return apiFetch<ArtifactSummary[]>("/api/workbench/artifacts");
}

/** Where artifacts are served, locally and through the relay. Not cached: the relay connection changes. */
export async function fetchWorkbenchInfo(): Promise<WorkbenchInfo> {
  return apiFetch<WorkbenchInfo>("/api/workbench/info");
}

/** Delete an artifact and its data files. Throws `ApiError` (404 if already gone). */
export async function deleteWorkbenchArtifact(name: string): Promise<void> {
  await apiFetch<unknown>(`/api/workbench/artifacts/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
}

// ── Workspace API wrappers ──────────────────────────────────────────

export async function fetchWorkspaceFiles(path?: string): Promise<WorkspaceEntry[]> {
  const params = path ? `?path=${encodeURIComponent(path)}` : "";
  return apiFetch<WorkspaceEntry[]>(`/api/workspace/files${params}`);
}

export async function fetchWorkspaceFile(path: string): Promise<string> {
  return apiFetchText(`/api/workspace/file?path=${encodeURIComponent(path)}`);
}

export async function putWorkspaceFile(
  path: string,
  content: string,
): Promise<WorkspaceWriteResponse> {
  return apiFetch<WorkspaceWriteResponse>("/api/workspace/file", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, content }),
  });
}

/** Diagnostics for `content` as if it were saved to `path`, without writing
 * anything. Empty means either the content is clean or `path` isn't one of
 * the strictly-parsed files the server checks. Graceful fallback: a network
 * or server error returns no diagnostics rather than interrupting typing. */
export async function validateWorkspaceFile(path: string, content: string): Promise<Diagnostic[]> {
  try {
    const result = await apiFetch<WorkspaceValidateResponse>("/api/workspace/validate", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ path, content }),
    });
    return result.diagnostics;
  } catch {
    return [];
  }
}

// ── Cloud API wrappers ──────────────────────────────────────────────

export async function fetchCloudStatus(): Promise<CloudStatusResponse> {
  return apiFetch<CloudStatusResponse>("/api/cloud/status");
}

export async function disconnectCloud(): Promise<void> {
  await apiFetchText("/api/cloud/disconnect", { method: "POST" });
}

// ── Update API wrappers ──────────────────────────────────────────────

export async function fetchUpdateStatus(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/api/update/status");
}

export async function triggerUpdateCheck(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/api/update/check", { method: "POST" });
}

export async function applyUpdate(): Promise<UpdateStatusResponse> {
  return apiFetch<UpdateStatusResponse>("/api/update/apply", { method: "POST" });
}
