// ── Typed fetch wrappers ─────────────────────────────────────────────

import type {
  StatusResponse,
  RecentMessage,
  TimezoneResponse,
  ModelsResponse,
  McpCatalogEntry,
  SecretResponse,
  ValidateResponse,
  SecretsListResponse,
  WorkspaceEntry,
  CloudStatusResponse,
} from "./types";

export async function fetchStatus(): Promise<StatusResponse> {
  const resp = await fetch("/api/status");
  if (!resp.ok) throw new Error(`status check failed: ${resp.status}`);
  return (await resp.json()) as StatusResponse;
}

export async function fetchChatHistory(): Promise<RecentMessage[]> {
  const resp = await fetch("/api/chat/history");
  if (!resp.ok) return [];
  return (await resp.json()) as RecentMessage[];
}

// ── Setup API wrappers ──────────────────────────────────────────────

export async function fetchTimezone(): Promise<string> {
  try {
    const resp = await fetch("/api/system/timezone");
    if (!resp.ok) throw new Error("failed");
    const data = (await resp.json()) as TimezoneResponse;
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

  const resp = await fetch("/api/providers/models", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return (await resp.json()) as ModelsResponse;
}

export async function fetchMcpCatalog(): Promise<McpCatalogEntry[]> {
  try {
    const resp = await fetch("/api/mcp-catalog");
    if (!resp.ok) return [];
    return (await resp.json()) as McpCatalogEntry[];
  } catch {
    return [];
  }
}

export async function storeSecret(name: string, value: string): Promise<SecretResponse> {
  const resp = await fetch("/api/secrets", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, value }),
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`failed to store secret "${name}": ${text}`);
  }
  return (await resp.json()) as SecretResponse;
}

export async function completeSetup(
  config: string,
  providers: string,
  mcpJson?: string,
): Promise<ValidateResponse> {
  const payload: Record<string, string> = { config, providers };
  if (mcpJson) payload.mcp_json = mcpJson;
  const resp = await fetch("/api/config/complete-setup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return (await resp.json()) as ValidateResponse;
}

// ── Settings API wrappers ────────────────────────────────────────────

export async function fetchConfigRaw(): Promise<string> {
  const resp = await fetch("/api/config/raw");
  if (!resp.ok) throw new Error(`failed to read config: ${resp.status}`);
  return resp.text();
}

export async function putConfigRaw(toml: string): Promise<ValidateResponse> {
  const resp = await fetch("/api/config/raw", {
    method: "PUT",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
  return (await resp.json()) as ValidateResponse;
}

export async function validateConfig(toml: string): Promise<ValidateResponse> {
  const resp = await fetch("/api/config/validate", {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
  return (await resp.json()) as ValidateResponse;
}

export async function fetchProvidersRaw(): Promise<string> {
  const resp = await fetch("/api/providers/raw");
  if (!resp.ok) throw new Error(`failed to read providers: ${resp.status}`);
  return resp.text();
}

export async function putProvidersRaw(toml: string): Promise<ValidateResponse> {
  const resp = await fetch("/api/providers/raw", {
    method: "PUT",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
  return (await resp.json()) as ValidateResponse;
}

export async function validateProviders(toml: string): Promise<ValidateResponse> {
  const resp = await fetch("/api/providers/validate", {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: toml,
  });
  return (await resp.json()) as ValidateResponse;
}

export async function fetchMcpRaw(): Promise<string> {
  const resp = await fetch("/api/mcp/raw");
  if (!resp.ok) throw new Error(`failed to read mcp.json: ${resp.status}`);
  return resp.text();
}

export async function putMcpRaw(json: string): Promise<ValidateResponse> {
  const resp = await fetch("/api/mcp/raw", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: json,
  });
  return (await resp.json()) as ValidateResponse;
}

export async function listSecrets(): Promise<string[]> {
  const resp = await fetch("/api/secrets");
  if (!resp.ok) return [];
  const data = (await resp.json()) as SecretsListResponse;
  return data.names;
}

export async function deleteSecret(name: string): Promise<void> {
  const resp = await fetch(`/api/secrets/${encodeURIComponent(name)}`, {
    method: "DELETE",
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`failed to delete secret "${name}": ${text}`);
  }
}

// ── Workspace API wrappers ──────────────────────────────────────────

export async function fetchWorkspaceFiles(path?: string): Promise<WorkspaceEntry[]> {
  const params = path ? `?path=${encodeURIComponent(path)}` : "";
  const resp = await fetch(`/api/workspace/files${params}`);
  if (!resp.ok) throw new Error(`failed to list workspace: ${resp.status}`);
  return (await resp.json()) as WorkspaceEntry[];
}

export async function fetchWorkspaceFile(path: string): Promise<string> {
  const resp = await fetch(`/api/workspace/file?path=${encodeURIComponent(path)}`);
  if (!resp.ok) throw new Error(`failed to read file: ${resp.status}`);
  return resp.text();
}

export async function putWorkspaceFile(path: string, content: string): Promise<void> {
  const resp = await fetch("/api/workspace/file", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, content }),
  });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`failed to save file: ${text}`);
  }
}

// ── Cloud API wrappers ──────────────────────────────────────────────

export async function fetchCloudStatus(): Promise<CloudStatusResponse> {
  const resp = await fetch("/api/cloud/status");
  if (!resp.ok) throw new Error(`cloud status check failed: ${resp.status}`);
  return (await resp.json()) as CloudStatusResponse;
}

export async function disconnectCloud(): Promise<void> {
  const resp = await fetch("/api/cloud/disconnect", { method: "POST" });
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`cloud disconnect failed: ${text}`);
  }
}
