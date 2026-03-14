// ── Settings TOML/JSON Parse & Serialize ─────────────────────────────
//
// Bidirectional conversion between raw config text and structured form state.
// Uses smol-toml for parsing and line-building for serialization (matching toml.ts).

import { parse as parseToml } from "smol-toml";
import type {
  McpServerEntry,
  SettingsProviderEntry,
  SettingsModelAssignments,
  ModelRoleKey,
  RoleOverrides,
} from "./types";

// ── Config form fields (config.toml) ─────────────────────────────────

export interface WebhookFormEntry {
  name: string;
  secret: string;
  routing: string;
  format: string;
  content_fields: string;
}

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
  // idle
  idle_timeout_minutes: string;
  idle_channel: string;
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
  // model parameters
  temperature: string;
  thinking: string;
  // integrations
  discord_token: string;
  telegram_token: string;
  webhooks: WebhookFormEntry[];
  // cloud
  cloud_enabled: boolean;
  cloud_token: string;
  cloud_relay_url: string;
  cloud_local_port: string;
  // skills
  skills_dirs: string[];
  // web search
  ws_backend: string;
  ws_brave_api_key: string;
  ws_tavily_api_key: string;
  ws_ollama_api_key: string;
  ws_ollama_base_url: string;
  ws_anthropic_max_uses: string;
  ws_anthropic_allowed_domains: string;
  ws_anthropic_blocked_domains: string;
  ws_openai_search_context_size: string;
  ws_gemini_exclude_domains: string;
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
    idle_timeout_minutes: "",
    idle_channel: "",
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
    temperature: "",
    thinking: "",
    discord_token: "",
    telegram_token: "",
    webhooks: [],
    cloud_enabled: true,
    cloud_token: "",
    cloud_relay_url: "",
    cloud_local_port: "",
    skills_dirs: [],
    ws_backend: "",
    ws_brave_api_key: "",
    ws_tavily_api_key: "",
    ws_ollama_api_key: "",
    ws_ollama_base_url: "",
    ws_anthropic_max_uses: "",
    ws_anthropic_allowed_domains: "",
    ws_anthropic_blocked_domains: "",
    ws_openai_search_context_size: "",
    ws_gemini_exclude_domains: "",
  };
}

// Helper to safely read nested TOML values
function str(v: unknown): string {
  if (v == null) return "";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v as string | number | boolean);
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
  fields.temperature = str(doc.temperature);
  fields.thinking = str(doc.thinking);

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

  const idle = doc.idle as Record<string, unknown> | undefined;
  if (idle) {
    fields.idle_timeout_minutes = str(idle.timeout_minutes);
    fields.idle_channel = str(idle.idle_channel);
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

  const webhooks = doc.webhooks as Record<string, Record<string, unknown>> | undefined;
  if (webhooks) {
    for (const [name, entry] of Object.entries(webhooks)) {
      fields.webhooks.push({
        name,
        secret: str(entry.secret),
        routing: str(entry.routing),
        format: str(entry.format),
        content_fields: Array.isArray(entry.content_fields)
          ? (entry.content_fields as string[]).join(", ")
          : "",
      });
    }
  }

  const cloud = doc.cloud as Record<string, unknown> | undefined;
  if (cloud) {
    fields.cloud_enabled = bool(cloud.enabled, true);
    fields.cloud_token = str(cloud.token);
    fields.cloud_relay_url = str(cloud.relay_url);
    fields.cloud_local_port = str(cloud.local_port);
  }

  const skills = doc.skills as Record<string, unknown> | undefined;
  if (skills && Array.isArray(skills.dirs)) {
    fields.skills_dirs = (skills.dirs as unknown[]).map(String);
  }

  const ws = doc.web_search as Record<string, unknown> | undefined;
  if (ws) {
    fields.ws_backend = str(ws.backend);
    const brave = ws.brave as Record<string, unknown> | undefined;
    if (brave) fields.ws_brave_api_key = str(brave.api_key);
    const tavily = ws.tavily as Record<string, unknown> | undefined;
    if (tavily) fields.ws_tavily_api_key = str(tavily.api_key);
    const ollama = ws.ollama as Record<string, unknown> | undefined;
    if (ollama) {
      fields.ws_ollama_api_key = str(ollama.api_key);
      fields.ws_ollama_base_url = str(ollama.base_url);
    }
    const anthropic = ws.anthropic as Record<string, unknown> | undefined;
    if (anthropic) {
      fields.ws_anthropic_max_uses = str(anthropic.max_uses);
      if (Array.isArray(anthropic.allowed_domains))
        fields.ws_anthropic_allowed_domains = (anthropic.allowed_domains as string[]).join(", ");
      if (Array.isArray(anthropic.blocked_domains))
        fields.ws_anthropic_blocked_domains = (anthropic.blocked_domains as string[]).join(", ");
    }
    const openai = ws.openai as Record<string, unknown> | undefined;
    if (openai) fields.ws_openai_search_context_size = str(openai.search_context_size);
    const gemini = ws.gemini as Record<string, unknown> | undefined;
    if (gemini && Array.isArray(gemini.exclude_domains))
      fields.ws_gemini_exclude_domains = (gemini.exclude_domains as string[]).join(", ");
  }

  return fields;
}

// ── Providers (providers.toml) ───────────────────────────────────────

export interface ProvidersFormState {
  providers: SettingsProviderEntry[];
  models: SettingsModelAssignments;
}

function defaultOverrides(): RoleOverrides {
  return { temperature: "", thinking: "" };
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
    overrides: {
      main: defaultOverrides(),
      default: defaultOverrides(),
      observer: defaultOverrides(),
      reflector: defaultOverrides(),
      pulse: defaultOverrides(),
      bgSmall: defaultOverrides(),
      bgMedium: defaultOverrides(),
      bgLarge: defaultOverrides(),
    },
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
        keepAlive: str(entry.keep_alive),
      });
    }
  }

  const models = doc.models as Record<string, unknown> | undefined;
  if (models) {
    for (const [tomlKey, formKey] of [
      ["main", "main"],
      ["default", "default"],
      ["observer", "observer"],
      ["reflector", "reflector"],
      ["pulse", "pulse"],
    ] as const) {
      const val = models[tomlKey];
      result.models[formKey as ModelRoleKey] = modelStr(val);
      extractOverrides(val, formKey, result.models.overrides);
    }
    result.models.embedding = str(models.embedding);
  }

  const bg = doc.background as Record<string, unknown> | undefined;
  if (bg) {
    const bgModels = bg.models as Record<string, unknown> | undefined;
    if (bgModels) {
      for (const [tomlKey, formKey] of [
        ["small", "bgSmall"],
        ["medium", "bgMedium"],
        ["large", "bgLarge"],
      ] as const) {
        const val = bgModels[tomlKey];
        result.models[formKey as ModelRoleKey] = modelStr(val);
        extractOverrides(val, formKey, result.models.overrides);
      }
    }
  }

  return result;
}

/** Model values can be a string, array (failover), or inline table. Show first entry for form. */
function modelStr(v: unknown): string {
  if (Array.isArray(v)) return v.length > 0 ? String(v[0]) : "";
  if (v == null) return "";
  if (typeof v === "object") {
    const obj = v as Record<string, unknown>;
    if ("model" in obj) {
      // Inline table form: { model = "...", temperature = ..., thinking = "..." }
      return modelStr(obj.model);
    }
    return JSON.stringify(v);
  }
  return String(v as string | number | boolean);
}

/** Extract temperature/thinking overrides from an inline table model assignment. */
function extractOverrides(v: unknown, key: string, overrides: Record<string, RoleOverrides>): void {
  if (v == null || typeof v !== "object" || Array.isArray(v)) return;
  const obj = v as Record<string, unknown>;
  if (!("model" in obj)) return;

  const temp = obj.temperature != null ? str(obj.temperature) : "";
  const thinking = obj.thinking != null ? str(obj.thinking) : "";
  if (temp || thinking) {
    overrides[key] ??= defaultOverrides();
    overrides[key].temperature = temp;
    overrides[key].thinking = thinking;
  }
}

// ── MCP (mcp.json) ──────────────────────────────────────────────────

export function parseMcpJson(raw: string): McpServerEntry[] {
  if (!raw.trim()) return [];
  try {
    const doc = JSON.parse(raw) as Record<string, unknown>;
    const servers = (doc.mcpServers as Record<string, Record<string, unknown>> | undefined) ?? {};
    return Object.entries(servers).map(([name, srv]) => ({
      name,
      command: str(srv.command),
      args: Array.isArray(srv.args) ? (srv.args as string[]) : [],
      env: (srv.env ?? {}) as Record<string, string>,
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
  if (f.temperature) lines.push(`temperature = ${f.temperature}`);
  if (f.thinking) lines.push(`thinking = "${f.thinking}"`);

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
  if (f.observer_threshold_tokens)
    memLines.push(`observer_threshold_tokens = ${f.observer_threshold_tokens}`);
  if (f.reflector_threshold_tokens)
    memLines.push(`reflector_threshold_tokens = ${f.reflector_threshold_tokens}`);
  if (f.observer_cooldown_secs)
    memLines.push(`observer_cooldown_secs = ${f.observer_cooldown_secs}`);
  if (f.observer_force_threshold_tokens)
    memLines.push(`observer_force_threshold_tokens = ${f.observer_force_threshold_tokens}`);

  const searchLines: string[] = [];
  if (f.search_vector_weight) searchLines.push(`vector_weight = ${f.search_vector_weight}`);
  if (f.search_text_weight) searchLines.push(`text_weight = ${f.search_text_weight}`);
  if (f.search_min_score) searchLines.push(`min_score = ${f.search_min_score}`);
  if (f.search_candidate_multiplier)
    searchLines.push(`candidate_multiplier = ${f.search_candidate_multiplier}`);
  if (f.search_temporal_decay) searchLines.push(`temporal_decay = true`);
  if (f.search_temporal_decay_half_life_days)
    searchLines.push(`temporal_decay_half_life_days = ${f.search_temporal_decay_half_life_days}`);

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

  // webhooks
  for (const wh of f.webhooks) {
    if (!wh.name.trim()) continue;
    lines.push("");
    lines.push(`[webhooks.${wh.name.trim()}]`);
    if (wh.secret) lines.push(`secret = "${wh.secret}"`);
    if (wh.routing && wh.routing !== "inbox") lines.push(`routing = "${wh.routing}"`);
    if (wh.format && wh.format !== "parsed") lines.push(`format = "${wh.format}"`);
    if (wh.content_fields) {
      const cfParts = wh.content_fields
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      if (cfParts.length > 0) {
        lines.push(`content_fields = [${cfParts.map((s) => `"${s}"`).join(", ")}]`);
      }
    }
  }

  // cloud
  if (f.cloud_token || f.cloud_relay_url || f.cloud_local_port || !f.cloud_enabled) {
    lines.push("");
    lines.push("[cloud]");
    if (!f.cloud_enabled) {
      lines.push("enabled = false");
    }
    if (f.cloud_token) lines.push(`token = "${f.cloud_token}"`);
    if (f.cloud_relay_url) lines.push(`relay_url = "${f.cloud_relay_url}"`);
    if (f.cloud_local_port) lines.push(`local_port = ${f.cloud_local_port}`);
  }

  // skills
  if (f.skills_dirs.length > 0) {
    lines.push("");
    lines.push("[skills]");
    const dirsStr = f.skills_dirs.map((d) => `"${d}"`).join(", ");
    lines.push(`dirs = [${dirsStr}]`);
  }

  // retry
  if (
    f.retry_max_retries ||
    f.retry_initial_delay_ms ||
    f.retry_max_delay_ms ||
    f.retry_backoff_multiplier
  ) {
    lines.push("");
    lines.push("[retry]");
    if (f.retry_max_retries) lines.push(`max_retries = ${f.retry_max_retries}`);
    if (f.retry_initial_delay_ms) lines.push(`initial_delay_ms = ${f.retry_initial_delay_ms}`);
    if (f.retry_max_delay_ms) lines.push(`max_delay_ms = ${f.retry_max_delay_ms}`);
    if (f.retry_backoff_multiplier)
      lines.push(`backoff_multiplier = ${f.retry_backoff_multiplier}`);
  }

  // background
  if (f.bg_max_concurrent || f.bg_transcript_retention_days) {
    lines.push("");
    lines.push("[background]");
    if (f.bg_max_concurrent) lines.push(`max_concurrent = ${f.bg_max_concurrent}`);
    if (f.bg_transcript_retention_days)
      lines.push(`transcript_retention_days = ${f.bg_transcript_retention_days}`);
  }

  // agent
  if (!f.agent_modify_mcp || !f.agent_modify_channels) {
    lines.push("");
    lines.push("[agent]");
    if (!f.agent_modify_mcp) lines.push("modify_mcp = false");
    if (!f.agent_modify_channels) lines.push("modify_channels = false");
  }

  // idle
  if (f.idle_timeout_minutes || f.idle_channel) {
    lines.push("");
    lines.push("[idle]");
    if (f.idle_timeout_minutes) lines.push(`timeout_minutes = ${f.idle_timeout_minutes}`);
    if (f.idle_channel) lines.push(`idle_channel = "${f.idle_channel}"`);
  }

  // web_search
  const hasWsBackend = Boolean(f.ws_backend);
  const hasWsNative =
    f.ws_anthropic_max_uses ||
    f.ws_anthropic_allowed_domains ||
    f.ws_anthropic_blocked_domains ||
    f.ws_openai_search_context_size ||
    f.ws_gemini_exclude_domains;

  if (hasWsBackend || hasWsNative) {
    lines.push("");
    lines.push("[web_search]");
    if (f.ws_backend) lines.push(`backend = "${f.ws_backend}"`);

    if (f.ws_backend === "brave" && f.ws_brave_api_key) {
      lines.push("");
      lines.push("[web_search.brave]");
      lines.push(`api_key = "${f.ws_brave_api_key}"`);
    }
    if (f.ws_backend === "tavily" && f.ws_tavily_api_key) {
      lines.push("");
      lines.push("[web_search.tavily]");
      lines.push(`api_key = "${f.ws_tavily_api_key}"`);
    }
    if (f.ws_backend === "ollama" && (f.ws_ollama_api_key || f.ws_ollama_base_url)) {
      lines.push("");
      lines.push("[web_search.ollama]");
      if (f.ws_ollama_api_key) lines.push(`api_key = "${f.ws_ollama_api_key}"`);
      if (f.ws_ollama_base_url) lines.push(`base_url = "${f.ws_ollama_base_url}"`);
    }
    if (
      f.ws_anthropic_max_uses ||
      f.ws_anthropic_allowed_domains ||
      f.ws_anthropic_blocked_domains
    ) {
      lines.push("");
      lines.push("[web_search.anthropic]");
      if (f.ws_anthropic_max_uses) lines.push(`max_uses = ${f.ws_anthropic_max_uses}`);
      if (f.ws_anthropic_allowed_domains) {
        const domains = f.ws_anthropic_allowed_domains
          .split(",")
          .map((d) => `"${d.trim()}"`)
          .filter((d) => d !== '""');
        if (domains.length > 0) lines.push(`allowed_domains = [${domains.join(", ")}]`);
      }
      if (f.ws_anthropic_blocked_domains) {
        const domains = f.ws_anthropic_blocked_domains
          .split(",")
          .map((d) => `"${d.trim()}"`)
          .filter((d) => d !== '""');
        if (domains.length > 0) lines.push(`blocked_domains = [${domains.join(", ")}]`);
      }
    }
    if (f.ws_openai_search_context_size) {
      lines.push("");
      lines.push("[web_search.openai]");
      lines.push(`search_context_size = "${f.ws_openai_search_context_size}"`);
    }
    if (f.ws_gemini_exclude_domains) {
      lines.push("");
      lines.push("[web_search.gemini]");
      const domains = f.ws_gemini_exclude_domains
        .split(",")
        .map((d) => `"${d.trim()}"`)
        .filter((d) => d !== '""');
      if (domains.length > 0) lines.push(`exclude_domains = [${domains.join(", ")}]`);
    }
  }

  lines.push("");
  return lines.join("\n");
}

/** Build a TOML line for a model role, using inline table when overrides exist. */
function serializeModelLine(
  tomlKey: string,
  modelValue: string,
  ov: RoleOverrides | undefined,
): string {
  const hasTemp = ov?.temperature != null && ov.temperature !== "";
  const hasThinking = ov?.thinking != null && ov.thinking !== "";
  if (!hasTemp && !hasThinking) {
    return `${tomlKey} = "${modelValue}"`;
  }
  const parts = [`model = "${modelValue}"`];
  if (hasTemp) parts.push(`temperature = ${ov.temperature}`);
  if (hasThinking) parts.push(`thinking = "${ov.thinking}"`);
  return `${tomlKey} = { ${parts.join(", ")} }`;
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
    if (p.apiKey) lines.push(`api_key = "${p.apiKey}"`);
    if (p.url) lines.push(`url = "${p.url}"`);
    if (p.keepAlive) lines.push(`keep_alive = "${p.keepAlive}"`);
    lines.push("");
  }

  // Models
  const modelLines: string[] = [];
  for (const [tomlKey, formKey] of [
    ["main", "main"],
    ["default", "default"],
    ["observer", "observer"],
    ["reflector", "reflector"],
    ["pulse", "pulse"],
  ] as const) {
    const val = models[formKey as ModelRoleKey];
    if (val) modelLines.push(serializeModelLine(tomlKey, val, models.overrides[formKey]));
  }
  if (models.embedding) modelLines.push(`embedding = "${models.embedding}"`);

  if (modelLines.length > 0) {
    lines.push("[models]");
    lines.push(...modelLines);
    lines.push("");
  }

  // Background models
  const bgLines: string[] = [];
  for (const [tomlKey, formKey] of [
    ["small", "bgSmall"],
    ["medium", "bgMedium"],
    ["large", "bgLarge"],
  ] as const) {
    const val = models[formKey as ModelRoleKey];
    if (val) bgLines.push(serializeModelLine(tomlKey, val, models.overrides[formKey]));
  }

  if (bgLines.length > 0) {
    lines.push("[background.models]");
    lines.push(...bgLines);
    lines.push("");
  }

  return lines.join("\n");
}

/** Serialize MCP servers back to mcp.json. */
export function serializeMcpJson(servers: McpServerEntry[]): string {
  const obj: Record<string, { command: string; args?: string[]; env?: Record<string, string> }> =
    {};
  for (const srv of servers) {
    const entry: { command: string; args?: string[]; env?: Record<string, string> } = {
      command: srv.command,
    };
    if (srv.args.length > 0) entry.args = srv.args;
    if (Object.keys(srv.env).length > 0) entry.env = srv.env;
    obj[srv.name] = entry;
  }
  return JSON.stringify({ mcpServers: obj }, null, 2) + "\n";
}
