// ── Setup wizard keys ─────────────────────────────────────────────────
//
// Which keys the setup wizard exchanges for secrets, the names they are
// stored under, and which providers and tool servers would be saved without
// one (a draft restored from localStorage never carries keys).

import type { McpCatalogEntry, ProviderKey, SetupMcpServer, SetupWizardState } from "./types";

/** The secret a tool server's key is stored under, e.g. `mcp_tavily_tavily_api_key`. */
export function mcpSecretName(server: string, envKey: string): string {
  return `mcp_${server}_${envKey.toLowerCase()}`;
}

/** The `env` keys a catalog server asks the user for (its fields without the `env.` prefix). */
export function catalogSecretEnvKeys(entry: McpCatalogEntry): string[] {
  return entry.requires_input.map((req) =>
    req.field.startsWith("env.") ? req.field.slice(4) : req.field,
  );
}

/**
 * Mark the keys of each server that doesn't say which of its `env` values are
 * keys, from the catalog entry of the same name. A draft saved before the
 * wizard recorded `secretEnvKeys` holds its keys in plain text; once they are
 * marked, the next draft save strips them and Save stores them as secrets.
 */
export function markCatalogKeys(servers: SetupMcpServer[], catalog: McpCatalogEntry[]): void {
  for (const srv of servers) {
    if (srv.secretEnvKeys !== undefined) continue;
    const entry = catalog.find((candidate) => candidate.name === srv.name);
    if (entry && entry.requires_input.length > 0) srv.secretEnvKeys = catalogSecretEnvKeys(entry);
  }
}

/** The `env` keys of `server` that hold a key and are empty. */
export function missingServerKeys(server: SetupMcpServer): string[] {
  return (server.secretEnvKeys ?? []).filter((key) => (server.env[key] ?? "").trim() === "");
}

/** Whether `provider` takes an API key and has none typed. */
export function providerMissingKey(state: SetupWizardState, provider: ProviderKey): boolean {
  return provider !== "ollama" && state.providerConfigs[provider].apiKey.trim() === "";
}

/** The selected providers and the tool servers that would be saved without a key. */
export interface KeylessEntries {
  providers: ProviderKey[];
  servers: string[];
}

export function keylessEntries(state: SetupWizardState): KeylessEntries {
  return {
    providers: state.selectedProviders.filter((prov) => providerMissingKey(state, prov)),
    servers: state.mcpServers
      .filter((srv) => missingServerKeys(srv).length > 0)
      .map((srv) => srv.name),
  };
}
