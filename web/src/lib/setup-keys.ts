// ── Setup wizard keys ─────────────────────────────────────────────────
//
// Which keys the setup wizard exchanges for secrets, the names they are
// stored under, and which providers and tool servers would be saved without
// one (a draft restored from localStorage never carries keys).

import type { McpServerEntry, ProviderKey, SetupWizardState } from "./types";

/** The secret a tool server's key is stored under, e.g. `mcp_tavily_tavily_api_key`. */
export function mcpSecretName(server: string, envKey: string): string {
  return `mcp_${server}_${envKey.toLowerCase()}`;
}

/** The `env` keys of `server` that hold a key and are empty. */
export function missingServerKeys(server: McpServerEntry): string[] {
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
