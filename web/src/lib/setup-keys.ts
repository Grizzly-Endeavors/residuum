import type { McpCatalogEntry, ProviderKey, SetupMcpServer, SetupWizardState } from "./types";

export function mcpSecretName(server: string, envKey: string): string {
  return `mcp_${server}_${envKey.toLowerCase()}`;
}

export function catalogSecretEnvKeys(entry: McpCatalogEntry): string[] {
  return entry.requires_input.map((req) =>
    req.field.startsWith("env.") ? req.field.slice(4) : req.field,
  );
}

export function markCatalogKeys(servers: SetupMcpServer[], catalog: McpCatalogEntry[]): void {
  for (const srv of servers) {
    if (srv.secretEnvKeys !== undefined) continue;
    const entry = catalog.find((candidate) => candidate.name === srv.name);
    if (entry && entry.requires_input.length > 0) srv.secretEnvKeys = catalogSecretEnvKeys(entry);
  }
}

export function missingServerKeys(server: SetupMcpServer): string[] {
  return (server.secretEnvKeys ?? []).filter((key) => (server.env[key] ?? "").trim() === "");
}

export function providerMissingKey(state: SetupWizardState, provider: ProviderKey): boolean {
  return provider !== "ollama" && state.providerConfigs[provider].apiKey.trim() === "";
}

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
