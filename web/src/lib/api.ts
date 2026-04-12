// ── Typed fetch wrappers ─────────────────────────────────────────────

import type {
  StatusResponse,
  ChatHistorySegment,
  RecentHistorySegment,
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

/** Fetch wrapper that throws `ApiError` on non-ok responses. */
async function apiFetch<T>(input: RequestInfo | URL, init?: RequestInit): Promise<T> {
  const resp = await fetch(input, init);
  if (!resp.ok) {
    const body = await resp.text();
    throw new ApiError(resp.status, resp.statusText, body);
  }
  return (await resp.json()) as T;
}

/** Fetch wrapper for plain text responses that throws `ApiError` on non-ok. */
async function apiFetchText(input: RequestInfo | URL, init?: RequestInit): Promise<string> {
  const resp = await fetch(input, init);
  if (!resp.ok) {
    const body = await resp.text();
    throw new ApiError(resp.status, resp.statusText, body);
  }
  return resp.text();
}

// ── Core API wrappers ───────────────────────────────────────────────

export async function fetchStatus(): Promise<StatusResponse> {
  return apiFetch<StatusResponse>("/api/status");
}

/** Graceful fallback: returns an empty Recent segment on failure. */
export async function fetchChatHistory(): Promise<RecentHistorySegment> {
  try {
    const segment = await apiFetch<ChatHistorySegment>("/api/chat/history");
    if (segment.kind === "recent") return segment;
    // Server should never return an Episode segment without an `episode` query
    // param, but fall back defensively.
    return { kind: "recent", messages: [], next_cursor: null };
  } catch {
    return { kind: "recent", messages: [], next_cursor: null };
  }
}

/** Fetch an episode segment by cursor. Returns null on 404 or error. */
export async function fetchChatSegment(episodeId: string): Promise<ChatHistorySegment | null> {
  try {
    return await apiFetch<ChatHistorySegment>(
      `/api/chat/history?episode=${encodeURIComponent(episodeId)}`,
    );
  } catch {
    return null;
  }
}

// ── Setup API wrappers ──────────────────────────────────────────────

/** Graceful fallback: uses browser timezone when server is unreachable during setup. */
export async function fetchTimezone(): Promise<string> {
  try {
    const data = await apiFetch<TimezoneResponse>("/api/system/timezone");
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
    return await apiFetch<McpCatalogEntry[]>("/api/mcp-catalog");
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
  return apiFetchText("/api/config/raw");
}

export async function putConfigRaw(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/config/raw", {
    method: "PUT",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function validateConfig(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/config/validate", {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchProvidersRaw(): Promise<string> {
  return apiFetchText("/api/providers/raw");
}

export async function putProvidersRaw(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/providers/raw", {
    method: "PUT",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function validateProviders(toml: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/providers/validate", {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
}

export async function fetchMcpRaw(): Promise<string> {
  return apiFetchText("/api/mcp/raw");
}

export async function putMcpRaw(json: string): Promise<ValidateResponse> {
  return apiFetch<ValidateResponse>("/api/mcp/raw", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: json,
  });
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
