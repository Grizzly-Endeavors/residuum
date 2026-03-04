// ── Settings TOML/JSON Parse & Serialize ─────────────────────────────
//
// Bidirectional conversion between raw config text and structured form state.
// Uses smol-toml for parsing and line-building for serialization (matching toml.ts).

import { parse as parseToml } from "smol-toml";
import type {
  McpServerEntry,
  SettingsProviderEntry,
  SettingsModelAssignments,
} from "./types";

// ── Config form fields (config.toml) ─────────────────────────────────

export interface ConfigFields {
  name: string;
  timezone: string;
  workspace_dir: string;
  timeout_secs: string;
  max_tokens: string;
  // gateway
  gateway_bind: string;
  gateway_port: string;
  // pulse
  pulse_enabled: boolean;
  // background
  bg_max_concurrent: string;
  bg_transcript_retention_days: string;
  // retry
  retry_max_retries: string;
  retry_initial_delay_ms: string;
  retry_max_delay_ms: string;
  retry_backoff_multiplier: string;
  // agent
  agent_modify_mcp: boolean;
  agent_modify_channels: boolean;
  // memory observation
  observer_threshold_tokens: string;
  reflector_threshold_tokens: string;
  observer_cooldown_secs: string;
  observer_force_threshold_tokens: string;
  // memory search
  search_vector_weight: string;
  search_text_weight: string;
  search_min_score: string;
  search_candidate_multiplier: string;
  search_temporal_decay: boolean;
  search_temporal_decay_half_life_days: string;
  // integrations
  discord_token: string;
  telegram_token: string;
  webhook_enabled: boolean;
  webhook_secret: string;
  // skills
  skills_dirs: string[];
}

export function defaultConfigFields(): ConfigFields {
  return {
    name: "",
    timezone: "",
    workspace_dir: "",
    timeout_secs: "",
    max_tokens: "",
    gateway_bind: "",
    gateway_port: "",
    pulse_enabled: true,
    bg_max_concurrent: "",
    bg_transcript_retention_days: "",
    retry_max_retries: "",
    retry_initial_delay_ms: "",
    retry_max_delay_ms: "",
    retry_backoff_multiplier: "",
    agent_modify_mcp: true,
    agent_modify_channels: true,
    observer_threshold_tokens: "",
    reflector_threshold_tokens: "",
    observer_cooldown_secs: "",
    observer_force_threshold_tokens: "",
    search_vector_weight: "",
    search_text_weight: "",
    search_min_score: "",
    search_candidate_multiplier: "",
    search_temporal_decay: false,
    search_temporal_decay_half_life_days: "",
    discord_token: "",
    telegram_token: "",
    webhook_enabled: false,
    webhook_secret: "",
    skills_dirs: [],
  };
}

// Helper to safely read nested TOML values
function str(v: unknown): string {
  return v == null ? "" : String(v);
}

function bool(v: unknown, fallback: boolean): boolean {
  return typeof v === "boolean" ? v : fallback;
}

export function parseConfigToml(raw: string): ConfigFields {
  const fields = defaultConfigFields();
  if (!raw.trim()) return fields;

  let doc: Record<string, unknown>;
  try {
    doc = parseToml(raw) as Record<string, unknown>;
  } catch {
    return fields;
  }

  fields.name = str(doc.name);
  fields.timezone = str(doc.timezone);
  fields.workspace_dir = str(doc.workspace_dir);
  fields.timeout_secs = str(doc.timeout_secs);
  fields.max_tokens = str(doc.max_tokens);

  const gw = doc.gateway as Record<string, unknown> | undefined;
  if (gw) {
    fields.gateway_bind = str(gw.bind);
    fields.gateway_port = str(gw.port);
  }

  const pulse = doc.pulse as Record<string, unknown> | undefined;
  if (pulse) {
    fields.pulse_enabled = bool(pulse.enabled, true);
  }

  const bg = doc.background as Record<string, unknown> | undefined;
  if (bg) {
    fields.bg_max_concurrent = str(bg.max_concurrent);
    fields.bg_transcript_retention_days = str(bg.transcript_retention_days);
  }

  const retry = doc.retry as Record<string, unknown> | undefined;
  if (retry) {
    fields.retry_max_retries = str(retry.max_retries);
    fields.retry_initial_delay_ms = str(retry.initial_delay_ms);
    fields.retry_max_delay_ms = str(retry.max_delay_ms);
    fields.retry_backoff_multiplier = str(retry.backoff_multiplier);
  }

  const agent = doc.agent as Record<string, unknown> | undefined;
  if (agent) {
    fields.agent_modify_mcp = bool(agent.modify_mcp, true);
    fields.agent_modify_channels = bool(agent.modify_channels, true);
  }

  const mem = doc.memory as Record<string, unknown> | undefined;
  if (mem) {
    fields.observer_threshold_tokens = str(mem.observer_threshold_tokens);
    fields.reflector_threshold_tokens = str(mem.reflector_threshold_tokens);
    fields.observer_cooldown_secs = str(mem.observer_cooldown_secs);
    fields.observer_force_threshold_tokens = str(mem.observer_force_threshold_tokens);
    const search = mem.search as Record<string, unknown> | undefined;
    if (search) {
      fields.search_vector_weight = str(search.vector_weight);
      fields.search_text_weight = str(search.text_weight);
      fields.search_min_score = str(search.min_score);
      fields.search_candidate_multiplier = str(search.candidate_multiplier);
      fields.search_temporal_decay = bool(search.temporal_decay, false);
      fields.search_temporal_decay_half_life_days = str(search.temporal_decay_half_life_days);
    }
  }

  const discord = doc.discord as Record<string, unknown> | undefined;
  if (discord) {
    fields.discord_token = str(discord.token);
  }

  const telegram = doc.telegram as Record<string, unknown> | undefined;
  if (telegram) {
    fields.telegram_token = str(telegram.token);
  }

  const webhook = doc.webhook as Record<string, unknown> | undefined;
  if (webhook) {
    fields.webhook_enabled = bool(webhook.enabled, false);
    fields.webhook_secret = str(webhook.secret);
  }

  const skills = doc.skills as Record<string, unknown> | undefined;
  if (skills && Array.isArray(skills.dirs)) {
    fields.skills_dirs = (skills.dirs as unknown[]).map(String);
  }

  return fields;
}

// ── Providers (providers.toml) ───────────────────────────────────────

export interface ProvidersFormState {
  providers: SettingsProviderEntry[];
  models: SettingsModelAssignments;
}

export function defaultModels(): SettingsModelAssignments {
  return {
    main: "",
    default: "",
    observer: "",
    reflector: "",
    pulse: "",
    embedding: "",
    bgSmall: "",
    bgMedium: "",
    bgLarge: "",
  };
}

export function parseProvidersToml(raw: string): ProvidersFormState {
  const result: ProvidersFormState = { providers: [], models: defaultModels() };
  if (!raw.trim()) return result;

  let doc: Record<string, unknown>;
  try {
    doc = parseToml(raw) as Record<string, unknown>;
  } catch {
    return result;
  }

  const provs = doc.providers as Record<string, Record<string, unknown>> | undefined;
  if (provs) {
    for (const [name, entry] of Object.entries(provs)) {
      result.providers.push({
        name,
        type: str(entry.type),
        apiKey: str(entry.api_key),
        url: str(entry.url),
      });
    }
  }

  const models = doc.models as Record<string, unknown> | undefined;
  if (models) {
    result.models.main = modelStr(models.main);
    result.models.default = modelStr(models.default);
    result.models.observer = modelStr(models.observer);
    result.models.reflector = modelStr(models.reflector);
    result.models.pulse = modelStr(models.pulse);
    result.models.embedding = str(models.embedding);
  }

  const bg = doc.background as Record<string, unknown> | undefined;
  if (bg) {
    const bgModels = bg.models as Record<string, unknown> | undefined;
    if (bgModels) {
      result.models.bgSmall = modelStr(bgModels.small);
      result.models.bgMedium = modelStr(bgModels.medium);
      result.models.bgLarge = modelStr(bgModels.large);
    }
  }

  return result;
}

/** Model values can be a string or array (failover). Show first entry for form. */
function modelStr(v: unknown): string {
  if (Array.isArray(v)) return v.length > 0 ? String(v[0]) : "";
  return v == null ? "" : String(v);
}

// ── MCP (mcp.json) ──────────────────────────────────────────────────

export function parseMcpJson(raw: string): McpServerEntry[] {
  if (!raw.trim()) return [];
  try {
    const doc = JSON.parse(raw) as Record<string, unknown>;
    const servers = (doc.mcpServers || {}) as Record<string, Record<string, unknown>>;
    return Object.entries(servers).map(([name, srv]) => ({
      name,
      command: str(srv.command),
      args: Array.isArray(srv.args) ? (srv.args as string[]) : [],
      env: (srv.env || {}) as Record<string, string>,
    }));
  } catch {
    return [];
  }
}

// ── Serializers ──────────────────────────────────────────────────────

/** Serialize config fields back to config.toml. */
export function serializeConfigToml(f: ConfigFields): string {
  const lines: string[] = [];

  if (f.name) lines.push(`name = "${f.name}"`);
  if (f.timezone) lines.push(`timezone = "${f.timezone}"`);
  if (f.workspace_dir) lines.push(`workspace_dir = "${f.workspace_dir}"`);
  if (f.timeout_secs) lines.push(`timeout_secs = ${f.timeout_secs}`);
  if (f.max_tokens) lines.push(`max_tokens = ${f.max_tokens}`);

  // gateway
  if (f.gateway_bind || f.gateway_port) {
    lines.push("");
    lines.push("[gateway]");
    if (f.gateway_bind) lines.push(`bind = "${f.gateway_bind}"`);
    if (f.gateway_port) lines.push(`port = ${f.gateway_port}`);
  }

  // pulse
  if (!f.pulse_enabled) {
    lines.push("");
    lines.push("[pulse]");
    lines.push("enabled = false");
  }

  // memory
  const memLines: string[] = [];
  if (f.observer_threshold_tokens) memLines.push(`observer_threshold_tokens = ${f.observer_threshold_tokens}`);
  if (f.reflector_threshold_tokens) memLines.push(`reflector_threshold_tokens = ${f.reflector_threshold_tokens}`);
  if (f.observer_cooldown_secs) memLines.push(`observer_cooldown_secs = ${f.observer_cooldown_secs}`);
  if (f.observer_force_threshold_tokens) memLines.push(`observer_force_threshold_tokens = ${f.observer_force_threshold_tokens}`);

  const searchLines: string[] = [];
  if (f.search_vector_weight) searchLines.push(`vector_weight = ${f.search_vector_weight}`);
  if (f.search_text_weight) searchLines.push(`text_weight = ${f.search_text_weight}`);
  if (f.search_min_score) searchLines.push(`min_score = ${f.search_min_score}`);
  if (f.search_candidate_multiplier) searchLines.push(`candidate_multiplier = ${f.search_candidate_multiplier}`);
  if (f.search_temporal_decay) searchLines.push(`temporal_decay = true`);
  if (f.search_temporal_decay_half_life_days) searchLines.push(`temporal_decay_half_life_days = ${f.search_temporal_decay_half_life_days}`);

  if (memLines.length > 0 || searchLines.length > 0) {
    lines.push("");
    lines.push("[memory]");
    lines.push(...memLines);
    if (searchLines.length > 0) {
      lines.push("");
      lines.push("[memory.search]");
      lines.push(...searchLines);
    }
  }

  // discord
  if (f.discord_token) {
    lines.push("");
    lines.push("[discord]");
    lines.push(`token = "${f.discord_token}"`);
  }

  // telegram
  if (f.telegram_token) {
    lines.push("");
    lines.push("[telegram]");
    lines.push(`token = "${f.telegram_token}"`);
  }

  // webhook
  if (f.webhook_enabled || f.webhook_secret) {
    lines.push("");
    lines.push("[webhook]");
    if (f.webhook_enabled) lines.push("enabled = true");
    if (f.webhook_secret) lines.push(`secret = "${f.webhook_secret}"`);
  }

  // skills
  if (f.skills_dirs.length > 0) {
    lines.push("");
    lines.push("[skills]");
    const dirsStr = f.skills_dirs.map((d) => `"${d}"`).join(", ");
    lines.push(`dirs = [${dirsStr}]`);
  }

  // retry
  if (f.retry_max_retries || f.retry_initial_delay_ms || f.retry_max_delay_ms || f.retry_backoff_multiplier) {
    lines.push("");
    lines.push("[retry]");
    if (f.retry_max_retries) lines.push(`max_retries = ${f.retry_max_retries}`);
    if (f.retry_initial_delay_ms) lines.push(`initial_delay_ms = ${f.retry_initial_delay_ms}`);
    if (f.retry_max_delay_ms) lines.push(`max_delay_ms = ${f.retry_max_delay_ms}`);
    if (f.retry_backoff_multiplier) lines.push(`backoff_multiplier = ${f.retry_backoff_multiplier}`);
  }

  // background
  if (f.bg_max_concurrent || f.bg_transcript_retention_days) {
    lines.push("");
    lines.push("[background]");
    if (f.bg_max_concurrent) lines.push(`max_concurrent = ${f.bg_max_concurrent}`);
    if (f.bg_transcript_retention_days) lines.push(`transcript_retention_days = ${f.bg_transcript_retention_days}`);
  }

  // agent
  if (!f.agent_modify_mcp || !f.agent_modify_channels) {
    lines.push("");
    lines.push("[agent]");
    if (!f.agent_modify_mcp) lines.push("modify_mcp = false");
    if (!f.agent_modify_channels) lines.push("modify_channels = false");
  }

  lines.push("");
  return lines.join("\n");
}

/** Serialize providers form state back to providers.toml. */
export function serializeProvidersToml(
  providers: SettingsProviderEntry[],
  models: SettingsModelAssignments,
): string {
  const lines: string[] = [];

  for (const p of providers) {
    lines.push(`[providers.${p.name}]`);
    lines.push(`type = "${p.type}"`);
    if (p.apiKey && p.type !== "ollama") lines.push(`api_key = "${p.apiKey}"`);
    if (p.url) lines.push(`url = "${p.url}"`);
    lines.push("");
  }

  // Models
  const modelLines: string[] = [];
  if (models.main) modelLines.push(`main = "${models.main}"`);
  if (models.default) modelLines.push(`default = "${models.default}"`);
  if (models.observer) modelLines.push(`observer = "${models.observer}"`);
  if (models.reflector) modelLines.push(`reflector = "${models.reflector}"`);
  if (models.pulse) modelLines.push(`pulse = "${models.pulse}"`);
  if (models.embedding) modelLines.push(`embedding = "${models.embedding}"`);

  if (modelLines.length > 0) {
    lines.push("[models]");
    lines.push(...modelLines);
    lines.push("");
  }

  // Background models
  const bgLines: string[] = [];
  if (models.bgSmall) bgLines.push(`small = "${models.bgSmall}"`);
  if (models.bgMedium) bgLines.push(`medium = "${models.bgMedium}"`);
  if (models.bgLarge) bgLines.push(`large = "${models.bgLarge}"`);

  if (bgLines.length > 0) {
    lines.push("[background.models]");
    lines.push(...bgLines);
    lines.push("");
  }

  return lines.join("\n");
}

/** Serialize MCP servers back to mcp.json. */
export function serializeMcpJson(servers: McpServerEntry[]): string {
  const obj: Record<string, { command: string; args?: string[]; env?: Record<string, string> }> = {};
  for (const srv of servers) {
    const entry: { command: string; args?: string[]; env?: Record<string, string> } = {
      command: srv.command,
    };
    if (srv.args && srv.args.length > 0) entry.args = srv.args;
    if (srv.env && Object.keys(srv.env).length > 0) entry.env = srv.env;
    obj[srv.name] = entry;
  }
  return JSON.stringify({ mcpServers: obj }, null, 2) + "\n";
}
