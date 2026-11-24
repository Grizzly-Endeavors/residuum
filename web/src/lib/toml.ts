import type { SetupWizardState } from "./types";
import { DEFAULT_MODELS } from "./models";
import { mcpSecretName } from "./setup-keys";

function escapeTomlString(s: string): string {
  return s.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

export function generateHubConfigToml(state: SetupWizardState): string {
  return `timezone = "${escapeTomlString(state.timezone)}"\n`;
}

export function generateConfigToml(state: SetupWizardState): string {
  const lines: string[] = [];

  if (state.integrations.discordToken) {
    const ref = state.secretRefs["discord"] ?? state.integrations.discordToken;
    lines.push("");
    lines.push("[discord]");
    lines.push(`token = "${escapeTomlString(ref)}"`);
  }

  if (state.integrations.telegramToken) {
    const ref = state.secretRefs["telegram"] ?? state.integrations.telegramToken;
    lines.push("");
    lines.push("[telegram]");
    lines.push(`token = "${escapeTomlString(ref)}"`);
  }

  if (
    state.integrations.teamsAppId &&
    state.integrations.teamsTenantId &&
    state.integrations.teamsAppPassword
  ) {
    const ref = state.secretRefs["teams"] ?? state.integrations.teamsAppPassword;
    lines.push("");
    lines.push("[teams]");
    lines.push(`app_id = "${escapeTomlString(state.integrations.teamsAppId)}"`);
    lines.push(`tenant_id = "${escapeTomlString(state.integrations.teamsTenantId)}"`);
    lines.push(`app_password = "${escapeTomlString(ref)}"`);
  }

  lines.push("");
  return lines.join("\n");
}

/** Generate providers.toml content (provider connections + model role assignments). */
export function generateProvidersToml(state: SetupWizardState): string {
  const lines: string[] = [];

  for (const prov of state.selectedProviders) {
    const cfg = state.providerConfigs[prov];
    lines.push(`[providers.${prov}]`);
    lines.push(`type = "${escapeTomlString(prov)}"`);
    if (prov !== "ollama") {
      const keyRef = state.secretRefs[prov] || cfg.apiKey;
      if (keyRef) lines.push(`api_key = "${escapeTomlString(keyRef)}"`);
      if (cfg.url) lines.push(`url = "${escapeTomlString(cfg.url)}"`);
    }
    lines.push("");
  }

  const mainCfg = state.providerConfigs[state.mainProvider];
  const mainModel = mainCfg.model || DEFAULT_MODELS[state.mainProvider] || "";
  lines.push("[models]");
  lines.push(`main = "${escapeTomlString(`${state.mainProvider}/${mainModel}`)}"`);

  for (const role of ["observer", "reflector", "pulse"]) {
    const r = state.roles[role];
    if (!r) continue;
    const prov = r.provider || state.mainProvider;
    if (r.model) {
      lines.push(`${role} = "${escapeTomlString(`${prov}/${r.model}`)}"`);
    }
  }

  if (state.embeddingModel.provider && state.embeddingModel.model) {
    lines.push(
      `embedding = "${escapeTomlString(`${state.embeddingModel.provider}/${state.embeddingModel.model}`)}"`,
    );
  }

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
      lines.push(`${tier} = "${escapeTomlString(`${prov}/${model}`)}"`);
    }
  }

  lines.push("");
  return lines.join("\n");
}

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
    const env = { ...srv.env };
    for (const key of srv.secretEnvKeys ?? []) {
      const ref = state.secretRefs[mcpSecretName(srv.name, key)];
      if (ref !== undefined) env[key] = ref;
    }
    if (Object.keys(env).length > 0) entry.env = env;
    servers[srv.name] = entry;
  }

  return JSON.stringify({ mcpServers: servers }, null, 2) + "\n";
}
