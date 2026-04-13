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
  WorkspaceEntry,
  CloudStatusResponse,
  UpdateStatusResponse,
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
  const result = await apiFetch<ValidateResponse>("/api/config/raw", {
    method: "PUT",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
  invalidate(CACHE_KEY_CONFIG_RAW);
  return result;
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
  const result = await apiFetch<ValidateResponse>("/api/providers/raw", {
    method: "PUT",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
  invalidate(CACHE_KEY_PROVIDERS_RAW);
  return result;
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
  const result = await apiFetch<ValidateResponse>("/api/mcp/raw", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: json,
  });
  invalidate(CACHE_KEY_MCP_RAW);
  return result;
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

// ── Workspace API wrappers ──────────────────────────────────────────

export async function fetchWorkspaceFiles(path?: string): Promise<WorkspaceEntry[]> {
  const params = path ? `?path=${encodeURIComponent(path)}` : "";
  return apiFetch<WorkspaceEntry[]>(`/api/workspace/files${params}`);
}

export async function fetchWorkspaceFile(path: string): Promise<string> {
  return apiFetchText(`/api/workspace/file?path=${encodeURIComponent(path)}`);
}

export async function putWorkspaceFile(path: string, content: string): Promise<void> {
  await apiFetch<unknown>("/api/workspace/file", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, content }),
  });
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
