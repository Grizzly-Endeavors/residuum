// ── Config Generators ───────────────────────────────────────────────
//
// Generates config.toml, providers.toml, and mcp.json from wizard state.

import type { SetupWizardState } from "./types";
import { DEFAULT_MODELS } from "./models";

/** Generate config.toml content (timezone, integrations). */
export function generateConfigToml(state: SetupWizardState): string {
  const lines: string[] = [];

  if (state.userName) lines.push(`name = "${state.userName}"`);
  lines.push(`timezone = "${state.timezone}"`);

  // Discord (top-level section)
  if (state.integrations.discordToken) {
    const ref = state.secretRefs["discord"] ?? state.integrations.discordToken;
    lines.push("");
    lines.push("[discord]");
    lines.push(`token = "${ref}"`);
  }

  // Telegram (top-level section)
  if (state.integrations.telegramToken) {
    const ref = state.secretRefs["telegram"] ?? state.integrations.telegramToken;
    lines.push("");
    lines.push("[telegram]");
    lines.push(`token = "${ref}"`);
  }

  lines.push("");
  return lines.join("\n");
}

/** Generate providers.toml content (provider connections + model role assignments). */
export function generateProvidersToml(state: SetupWizardState): string {
  const lines: string[] = [];

  // Collect provider entries
  const providerEntries: Record<string, { type: string; api_key: string; url: string | null }> = {};

  for (const prov of state.selectedProviders) {
    const cfg = state.providerConfigs[prov];
    if (prov === "ollama") {
      providerEntries[prov] = { type: prov, api_key: "", url: null };
    } else if (cfg.apiKey) {
      providerEntries[prov] = {
        type: prov,
        api_key: cfg.apiKey,
        url: cfg.url || null,
      };
    }
  }

  // Role providers (if different from selected ones)
  for (const role of ["observer", "reflector", "pulse"]) {
    const r = state.roles[role];
    if (!r) continue;
    const prov = r.provider || state.mainProvider;
    if (!providerEntries[prov] && prov !== "ollama" && r.apiKey) {
      providerEntries[prov] = {
        type: prov,
        api_key: r.apiKey,
        url: null,
      };
    }
  }

  // Write provider entries
  for (const [name, cfg] of Object.entries(providerEntries)) {
    if (name === "ollama") {
      lines.push(`[providers.${name}]`);
      lines.push(`type = "${cfg.type}"`);
      lines.push("");
      continue;
    }
    lines.push(`[providers.${name}]`);
    lines.push(`type = "${cfg.type}"`);
    const keyRef = state.secretRefs[name] ?? cfg.api_key;
    lines.push(`api_key = "${keyRef}"`);
    if (cfg.url) lines.push(`url = "${cfg.url}"`);
    lines.push("");
  }

  // Models section
  const mainCfg = state.providerConfigs[state.mainProvider];
  const mainModel = mainCfg.model || DEFAULT_MODELS[state.mainProvider] || "";
  lines.push("[models]");
  lines.push(`main = "${state.mainProvider}/${mainModel}"`);

  for (const role of ["observer", "reflector", "pulse"]) {
    const r = state.roles[role];
    if (!r) continue;
    const prov = r.provider || state.mainProvider;
    if (r.model) {
      lines.push(`${role} = "${prov}/${r.model}"`);
    }
  }

  // Embedding model
  if (state.embeddingModel.provider && state.embeddingModel.model) {
    lines.push(`embedding = "${state.embeddingModel.provider}/${state.embeddingModel.model}"`);
  }

  // Background model tiers (lives in providers.toml alongside other model config)
  const bgEntries: { tier: string; prov: string; model: string }[] = [];
  for (const tier of ["small", "medium", "large"]) {
    const bg = state.backgroundModels[tier];
    if (!bg) continue;
    const prov = bg.provider || state.mainProvider;
    if (bg.model) {
      bgEntries.push({ tier, prov, model: bg.model });
    }
  }
  if (bgEntries.length > 0) {
    lines.push("");
    lines.push("[background.models]");
    for (const { tier, prov, model } of bgEntries) {
      lines.push(`${tier} = "${prov}/${model}"`);
    }
  }

  lines.push("");
  return lines.join("\n");
}

/** Generate mcp.json content (Claude Code format). */
export function generateMcpJson(state: SetupWizardState): string {
  const servers: Record<
    string,
    { command: string; args?: string[]; env?: Record<string, string> }
  > = {};

  for (const srv of state.mcpServers) {
    const entry: { command: string; args?: string[]; env?: Record<string, string> } = {
      command: srv.command,
    };
    if (srv.args.length > 0) entry.args = srv.args;
    if (Object.keys(srv.env).length > 0) entry.env = srv.env;
    servers[srv.name] = entry;
  }

  return JSON.stringify({ mcpServers: servers }, null, 2) + "\n";
}
