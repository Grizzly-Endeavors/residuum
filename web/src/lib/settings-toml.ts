// ── Settings TOML/JSON Parse & Diff ──────────────────────────────────
//
// Parsing raw config text into structured form state (for display), and
// diffing two structured snapshots into the JSON patch shape the server's
// `PATCH /api/config/patch`, `/api/providers/patch`, and `/api/mcp/patch`
// endpoints expect. The form never rebuilds a whole file from its own
// state — only the fields it diffs as changed are sent, so anything the
// form doesn't model (comments, unmodeled sections/keys, HTTP MCP server
// fields untouched by the edit) survives on the server.
//
// Patch value conventions (mirrored by `src/config/patch.rs` and
// `src/workspace/mcp_patch.rs`):
//   - a nested object recurses into (or creates) the matching TOML/JSON table
//   - `null` removes that key, or a whole named entry (provider/webhook/MCP
//     server) when the value is a whole entry rather than a single field
//   - `{"$inline": {...}}` sets a model-role key to a TOML inline table
//     (used for `temperature`/`thinking` overrides)
//   - anything else is a plain scalar or array value

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
  // subconscious
  subconscious_enabled: boolean;
  subconscious_mid_turn: boolean;
  subconscious_every_n_iterations: string;
  subconscious_max_transcript_tokens: string;
  subconscious_learning: boolean;
  subconscious_learning_cooldown_minutes: string;
  // learning fallback
  learning_nudge_after_turns: string;
  // background
  bg_max_concurrent: string;
  bg_idle_timeout_scheduled_minutes: string;
  bg_idle_timeout_spawned_minutes: string;
  bg_idle_timeout_external_minutes: string;
  bg_idle_timeout_artifact_minutes: string;
  bg_episode_skip_token_floor: string;
  bg_subagent_depth_cap: string;
  bg_hop_soft_limit: string;
  bg_hop_hard_limit: string;
  // retry
  retry_max_retries: string;
  retry_initial_delay_ms: string;
  retry_max_delay_ms: string;
  retry_backoff_multiplier: string;
  // agent
  agent_modify_mcp: boolean;
  agent_modify_channels: boolean;
  agent_max_tool_iterations: string;
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
  discord_respond_to_others: boolean;
  discord_context_messages: string;
  telegram_token: string;
  telegram_respond_to_others: boolean;
  telegram_context_messages: string;
  teams_app_id: string;
  teams_tenant_id: string;
  teams_app_password: string;
  teams_respond_to_others: boolean;
  teams_context_messages: string;
  teams_port: string;
  a2a_enabled: boolean;
  a2a_port: string;
  a2a_public_url: string;
  a2a_visibility: string;
  webhooks: WebhookFormEntry[];
  // cloud
  cloud_enabled: boolean;
  cloud_token: string;
  cloud_relay_url: string;
  cloud_local_port: string;
  // skills
  skills_dirs: string[];
  // tools
  tools_path: string[];
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
    subconscious_enabled: false,
    subconscious_mid_turn: true,
    subconscious_every_n_iterations: "",
    subconscious_max_transcript_tokens: "",
    subconscious_learning: false,
    subconscious_learning_cooldown_minutes: "",
    learning_nudge_after_turns: "",
    bg_max_concurrent: "",
    bg_idle_timeout_scheduled_minutes: "",
    bg_idle_timeout_spawned_minutes: "",
    bg_idle_timeout_external_minutes: "",
    bg_idle_timeout_artifact_minutes: "",
    bg_episode_skip_token_floor: "",
    bg_subagent_depth_cap: "",
    bg_hop_soft_limit: "",
    bg_hop_hard_limit: "",
    retry_max_retries: "",
    retry_initial_delay_ms: "",
    retry_max_delay_ms: "",
    retry_backoff_multiplier: "",
    agent_modify_mcp: true,
    agent_modify_channels: true,
    agent_max_tool_iterations: "",
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
    discord_respond_to_others: false,
    discord_context_messages: "",
    telegram_token: "",
    telegram_respond_to_others: false,
    telegram_context_messages: "",
    teams_app_id: "",
    teams_tenant_id: "",
    teams_app_password: "",
    teams_respond_to_others: false,
    teams_context_messages: "",
    teams_port: "",
    a2a_enabled: true,
    a2a_port: "",
    a2a_public_url: "",
    a2a_visibility: "",
    webhooks: [],
    cloud_enabled: true,
    cloud_token: "",
    cloud_relay_url: "",
    cloud_local_port: "",
    skills_dirs: [],
    tools_path: [],
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
  if (typeof v === "object") {
    // eslint-disable-next-line no-console -- parser-layer failure has no user-visible channel; surfaces config drift
    console.warn("unexpected object in toml scalar field", v);
    return "";
  }
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

  const subconscious = doc.subconscious as Record<string, unknown> | undefined;
  if (subconscious) {
    fields.subconscious_enabled = bool(subconscious.enabled, false);
    fields.subconscious_mid_turn = bool(subconscious.mid_turn, true);
    fields.subconscious_every_n_iterations = str(subconscious.every_n_iterations);
    fields.subconscious_max_transcript_tokens = str(subconscious.max_transcript_tokens);
    fields.subconscious_learning = bool(subconscious.learning, false);
    fields.subconscious_learning_cooldown_minutes = str(subconscious.learning_cooldown_minutes);
  }

  const learning = doc.learning as Record<string, unknown> | undefined;
  if (learning) {
    fields.learning_nudge_after_turns = str(learning.nudge_after_turns);
  }

  const bg = doc.background as Record<string, unknown> | undefined;
  if (bg) {
    fields.bg_max_concurrent = str(bg.max_concurrent);
    fields.bg_idle_timeout_scheduled_minutes = str(bg.idle_timeout_scheduled_minutes);
    fields.bg_idle_timeout_spawned_minutes = str(bg.idle_timeout_spawned_minutes);
    fields.bg_idle_timeout_external_minutes = str(bg.idle_timeout_external_minutes);
    fields.bg_idle_timeout_artifact_minutes = str(bg.idle_timeout_artifact_minutes);
    fields.bg_episode_skip_token_floor = str(bg.episode_skip_token_floor);
    fields.bg_subagent_depth_cap = str(bg.subagent_depth_cap);
    fields.bg_hop_soft_limit = str(bg.hop_soft_limit);
    fields.bg_hop_hard_limit = str(bg.hop_hard_limit);
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
    fields.agent_max_tool_iterations = str(agent.max_tool_iterations);
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
    fields.discord_respond_to_others = bool(discord.respond_to_others, false);
    fields.discord_context_messages = str(discord.context_messages);
  }

  const telegram = doc.telegram as Record<string, unknown> | undefined;
  if (telegram) {
    fields.telegram_token = str(telegram.token);
    fields.telegram_respond_to_others = bool(telegram.respond_to_others, false);
    fields.telegram_context_messages = str(telegram.context_messages);
  }

  const teams = doc.teams as Record<string, unknown> | undefined;
  if (teams) {
    fields.teams_app_id = str(teams.app_id);
    fields.teams_tenant_id = str(teams.tenant_id);
    fields.teams_app_password = str(teams.app_password);
    fields.teams_respond_to_others = bool(teams.respond_to_others, false);
    fields.teams_context_messages = str(teams.context_messages);
    fields.teams_port = str(teams.port);
  }

  const a2a = doc.a2a as Record<string, unknown> | undefined;
  if (a2a) {
    fields.a2a_enabled = bool(a2a.enabled, true);
    fields.a2a_port = str(a2a.port);
    fields.a2a_public_url = str(a2a.public_url);
    fields.a2a_visibility = str(a2a.visibility);
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

  const tools = doc.tools as Record<string, unknown> | undefined;
  if (tools && Array.isArray(tools.path)) {
    fields.tools_path = (tools.path as unknown[]).map(String);
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
    subconscious: "",
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
      subconscious: defaultOverrides(),
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
      ["subconscious", "subconscious"],
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

/**
 * Parse `mcp.json` into form entries, representing each server's transport
 * truthfully. Mirrors the backend loader's transport resolution
 * (`src/workspace/config.rs`): `type` takes priority over `transport`;
 * `"streamable-http"`/`"http"` is HTTP, everything else (including a
 * missing field) is stdio. An HTTP server's URL falls back to `command`
 * for display, matching the loader.
 */
export function parseMcpJson(raw: string): McpServerEntry[] {
  if (!raw.trim()) return [];
  try {
    const doc = JSON.parse(raw) as Record<string, unknown>;
    const servers = (doc.mcpServers as Record<string, Record<string, unknown>> | undefined) ?? {};
    return Object.entries(servers).map(([name, srv]) => {
      const typeField = typeof srv.type === "string" ? srv.type : undefined;
      const transportField = typeof srv.transport === "string" ? srv.transport : undefined;
      const kind = typeField ?? transportField;
      const transport: "stdio" | "http" =
        kind === "http" || kind === "streamable-http" ? "http" : "stdio";
      const command = str(srv.command);
      const url = str(srv.url);
      return {
        name,
        transport,
        command: transport === "http" ? "" : command,
        args: Array.isArray(srv.args) ? (srv.args as string[]) : [],
        env: (srv.env ?? {}) as Record<string, string>,
        url: transport === "http" ? url || command : "",
        headers: (srv.headers ?? {}) as Record<string, string>,
      };
    });
  } catch {
    return [];
  }
}

// ── Diff helpers ───────────────────────────────────────────────────────

/** Split a comma-separated user input into a cleaned string list (no empties). */
function commaList(raw: string): string[] {
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/** Parse a numeric form field the way a bare TOML literal would: a float if it has a decimal point, else an int. */
function numberLiteral(raw: string): number {
  return raw.includes(".") ? parseFloat(raw) : parseInt(raw, 10);
}

function jsonEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Nest `value` into `root` at `path`, creating intermediate objects as needed. */
function setPath(root: Record<string, unknown>, path: readonly string[], value: unknown): void {
  let node = root;
  for (let i = 0; i < path.length - 1; i++) {
    const seg = path[i];
    if (seg === undefined) continue;
    const existing = node[seg];
    if (typeof existing !== "object" || existing === null || Array.isArray(existing)) {
      node[seg] = {};
    }
    node = node[seg] as Record<string, unknown>;
  }
  const last = path[path.length - 1];
  if (last !== undefined) node[last] = value;
}

type FieldSpec =
  | { key: keyof ConfigFields; path: readonly string[]; kind: "string" }
  | { key: keyof ConfigFields; path: readonly string[]; kind: "stringDefault"; default: string }
  | { key: keyof ConfigFields; path: readonly string[]; kind: "number" }
  | { key: keyof ConfigFields; path: readonly string[]; kind: "numberDefault"; default: string }
  | { key: keyof ConfigFields; path: readonly string[]; kind: "bool"; default: boolean }
  | { key: keyof ConfigFields; path: readonly string[]; kind: "stringArray" }
  | { key: keyof ConfigFields; path: readonly string[]; kind: "commaList" };

/** Canonicalize a raw form value into the JSON shape it patches to, or `null` when it should be absent (empty/default). */
function canonField(spec: FieldSpec, raw: unknown): unknown {
  switch (spec.kind) {
    case "string": {
      const s = (raw as string | undefined) ?? "";
      return s.trim() ? s : null;
    }
    case "stringDefault": {
      const s = (raw as string | undefined) ?? "";
      return !s || s === spec.default ? null : s;
    }
    case "number": {
      const s = (raw as string | undefined) ?? "";
      return s.trim() ? numberLiteral(s) : null;
    }
    case "numberDefault": {
      const s = (raw as string | undefined) ?? "";
      return !s || s === spec.default ? null : numberLiteral(s);
    }
    case "bool": {
      const b = (raw as boolean | undefined) ?? spec.default;
      return b === spec.default ? null : b;
    }
    case "stringArray": {
      const arr = (raw as string[] | undefined) ?? [];
      return arr.length ? arr : null;
    }
    case "commaList": {
      const parts = commaList((raw as string | undefined) ?? "");
      return parts.length ? parts : null;
    }
  }
}

const CONFIG_FIELD_MAP: readonly FieldSpec[] = [
  { key: "name", path: ["name"], kind: "string" },
  { key: "timezone", path: ["timezone"], kind: "string" },
  { key: "workspace_dir", path: ["workspace_dir"], kind: "string" },
  { key: "timeout_secs", path: ["timeout_secs"], kind: "number" },
  { key: "max_tokens", path: ["max_tokens"], kind: "number" },
  { key: "temperature", path: ["temperature"], kind: "number" },
  { key: "thinking", path: ["thinking"], kind: "string" },

  { key: "gateway_bind", path: ["gateway", "bind"], kind: "string" },
  { key: "gateway_port", path: ["gateway", "port"], kind: "number" },

  { key: "pulse_enabled", path: ["pulse", "enabled"], kind: "bool", default: true },

  { key: "subconscious_enabled", path: ["subconscious", "enabled"], kind: "bool", default: false },
  { key: "subconscious_mid_turn", path: ["subconscious", "mid_turn"], kind: "bool", default: true },
  {
    key: "subconscious_every_n_iterations",
    path: ["subconscious", "every_n_iterations"],
    kind: "number",
  },
  {
    key: "subconscious_max_transcript_tokens",
    path: ["subconscious", "max_transcript_tokens"],
    kind: "number",
  },
  {
    key: "subconscious_learning",
    path: ["subconscious", "learning"],
    kind: "bool",
    default: false,
  },
  {
    key: "subconscious_learning_cooldown_minutes",
    path: ["subconscious", "learning_cooldown_minutes"],
    kind: "number",
  },

  { key: "learning_nudge_after_turns", path: ["learning", "nudge_after_turns"], kind: "number" },

  { key: "bg_max_concurrent", path: ["background", "max_concurrent"], kind: "number" },
  {
    key: "bg_idle_timeout_scheduled_minutes",
    path: ["background", "idle_timeout_scheduled_minutes"],
    kind: "number",
  },
  {
    key: "bg_idle_timeout_spawned_minutes",
    path: ["background", "idle_timeout_spawned_minutes"],
    kind: "number",
  },
  {
    key: "bg_idle_timeout_external_minutes",
    path: ["background", "idle_timeout_external_minutes"],
    kind: "number",
  },
  {
    key: "bg_idle_timeout_artifact_minutes",
    path: ["background", "idle_timeout_artifact_minutes"],
    kind: "number",
  },
  {
    key: "bg_episode_skip_token_floor",
    path: ["background", "episode_skip_token_floor"],
    kind: "number",
  },
  { key: "bg_subagent_depth_cap", path: ["background", "subagent_depth_cap"], kind: "number" },
  { key: "bg_hop_soft_limit", path: ["background", "hop_soft_limit"], kind: "number" },
  { key: "bg_hop_hard_limit", path: ["background", "hop_hard_limit"], kind: "number" },

  { key: "retry_max_retries", path: ["retry", "max_retries"], kind: "number" },
  { key: "retry_initial_delay_ms", path: ["retry", "initial_delay_ms"], kind: "number" },
  { key: "retry_max_delay_ms", path: ["retry", "max_delay_ms"], kind: "number" },
  { key: "retry_backoff_multiplier", path: ["retry", "backoff_multiplier"], kind: "number" },

  { key: "agent_modify_mcp", path: ["agent", "modify_mcp"], kind: "bool", default: true },
  { key: "agent_modify_channels", path: ["agent", "modify_channels"], kind: "bool", default: true },
  { key: "agent_max_tool_iterations", path: ["agent", "max_tool_iterations"], kind: "number" },

  { key: "idle_timeout_minutes", path: ["idle", "timeout_minutes"], kind: "number" },
  { key: "idle_channel", path: ["idle", "idle_channel"], kind: "string" },

  {
    key: "observer_threshold_tokens",
    path: ["memory", "observer_threshold_tokens"],
    kind: "number",
  },
  {
    key: "reflector_threshold_tokens",
    path: ["memory", "reflector_threshold_tokens"],
    kind: "number",
  },
  { key: "observer_cooldown_secs", path: ["memory", "observer_cooldown_secs"], kind: "number" },
  {
    key: "observer_force_threshold_tokens",
    path: ["memory", "observer_force_threshold_tokens"],
    kind: "number",
  },
  { key: "search_vector_weight", path: ["memory", "search", "vector_weight"], kind: "number" },
  { key: "search_text_weight", path: ["memory", "search", "text_weight"], kind: "number" },
  { key: "search_min_score", path: ["memory", "search", "min_score"], kind: "number" },
  {
    key: "search_candidate_multiplier",
    path: ["memory", "search", "candidate_multiplier"],
    kind: "number",
  },
  {
    key: "search_temporal_decay",
    path: ["memory", "search", "temporal_decay"],
    kind: "bool",
    default: false,
  },
  {
    key: "search_temporal_decay_half_life_days",
    path: ["memory", "search", "temporal_decay_half_life_days"],
    kind: "number",
  },

  { key: "discord_token", path: ["discord", "token"], kind: "string" },
  {
    key: "discord_respond_to_others",
    path: ["discord", "respond_to_others"],
    kind: "bool",
    default: false,
  },
  {
    key: "discord_context_messages",
    path: ["discord", "context_messages"],
    kind: "numberDefault",
    default: "20",
  },

  { key: "telegram_token", path: ["telegram", "token"], kind: "string" },
  {
    key: "telegram_respond_to_others",
    path: ["telegram", "respond_to_others"],
    kind: "bool",
    default: false,
  },
  {
    key: "telegram_context_messages",
    path: ["telegram", "context_messages"],
    kind: "numberDefault",
    default: "20",
  },

  { key: "teams_app_id", path: ["teams", "app_id"], kind: "string" },
  { key: "teams_tenant_id", path: ["teams", "tenant_id"], kind: "string" },
  { key: "teams_app_password", path: ["teams", "app_password"], kind: "string" },
  {
    key: "teams_respond_to_others",
    path: ["teams", "respond_to_others"],
    kind: "bool",
    default: false,
  },
  {
    key: "teams_context_messages",
    path: ["teams", "context_messages"],
    kind: "numberDefault",
    default: "20",
  },
  { key: "teams_port", path: ["teams", "port"], kind: "numberDefault", default: "7701" },

  { key: "a2a_enabled", path: ["a2a", "enabled"], kind: "bool", default: true },
  { key: "a2a_port", path: ["a2a", "port"], kind: "numberDefault", default: "7702" },
  { key: "a2a_public_url", path: ["a2a", "public_url"], kind: "string" },
  { key: "a2a_visibility", path: ["a2a", "visibility"], kind: "stringDefault", default: "public" },

  { key: "cloud_enabled", path: ["cloud", "enabled"], kind: "bool", default: true },
  { key: "cloud_token", path: ["cloud", "token"], kind: "string" },
  { key: "cloud_relay_url", path: ["cloud", "relay_url"], kind: "string" },
  { key: "cloud_local_port", path: ["cloud", "local_port"], kind: "number" },

  { key: "skills_dirs", path: ["skills", "dirs"], kind: "stringArray" },
  { key: "tools_path", path: ["tools", "path"], kind: "stringArray" },

  { key: "ws_backend", path: ["web_search", "backend"], kind: "string" },
  { key: "ws_brave_api_key", path: ["web_search", "brave", "api_key"], kind: "string" },
  { key: "ws_tavily_api_key", path: ["web_search", "tavily", "api_key"], kind: "string" },
  { key: "ws_ollama_api_key", path: ["web_search", "ollama", "api_key"], kind: "string" },
  { key: "ws_ollama_base_url", path: ["web_search", "ollama", "base_url"], kind: "string" },
  { key: "ws_anthropic_max_uses", path: ["web_search", "anthropic", "max_uses"], kind: "number" },
  {
    key: "ws_anthropic_allowed_domains",
    path: ["web_search", "anthropic", "allowed_domains"],
    kind: "commaList",
  },
  {
    key: "ws_anthropic_blocked_domains",
    path: ["web_search", "anthropic", "blocked_domains"],
    kind: "commaList",
  },
  {
    key: "ws_openai_search_context_size",
    path: ["web_search", "openai", "search_context_size"],
    kind: "string",
  },
  {
    key: "ws_gemini_exclude_domains",
    path: ["web_search", "gemini", "exclude_domains"],
    kind: "commaList",
  },
];

type WebhookFieldSpec =
  | { key: keyof WebhookFormEntry; kind: "string" }
  | { key: keyof WebhookFormEntry; kind: "stringDefault"; default: string }
  | { key: keyof WebhookFormEntry; kind: "commaList" };

const WEBHOOK_FIELD_MAP: readonly WebhookFieldSpec[] = [
  { key: "secret", kind: "string" },
  { key: "routing", kind: "stringDefault", default: "inbox" },
  { key: "format", kind: "stringDefault", default: "parsed" },
  { key: "content_fields", kind: "commaList" },
];

function canonWebhookField(spec: WebhookFieldSpec, raw: string): unknown {
  switch (spec.kind) {
    case "string":
      return raw.trim() ? raw : null;
    case "stringDefault":
      return !raw || raw === spec.default ? null : raw;
    case "commaList": {
      const parts = commaList(raw);
      return parts.length ? parts : null;
    }
  }
}

function webhookToJson(wh: WebhookFormEntry): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const spec of WEBHOOK_FIELD_MAP) {
    const val = canonWebhookField(spec, wh[spec.key]);
    if (val !== null) out[spec.key] = val;
  }
  return out;
}

function diffWebhookFields(
  base: WebhookFormEntry,
  current: WebhookFormEntry,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const spec of WEBHOOK_FIELD_MAP) {
    const before = canonWebhookField(spec, base[spec.key]);
    const after = canonWebhookField(spec, current[spec.key]);
    if (!jsonEqual(before, after)) out[spec.key] = after;
  }
  return out;
}

/** Diff a named collection (webhooks/providers/mcp servers), keyed by name. */
function diffNamedCollection<T extends { name: string }>(
  baseline: readonly T[],
  current: readonly T[],
  toJson: (entry: T) => Record<string, unknown>,
  diffFields: (base: T, current: T) => Record<string, unknown>,
): { patch: Record<string, unknown>; touched: boolean } {
  const baseByName = new Map(baseline.filter((e) => e.name.trim()).map((e) => [e.name.trim(), e]));
  const seen = new Set<string>();
  const patch: Record<string, unknown> = {};
  let touched = false;

  for (const entry of current) {
    const name = entry.name.trim();
    if (!name) continue;
    seen.add(name);
    const base = baseByName.get(name);
    if (!base) {
      patch[name] = toJson(entry);
      touched = true;
      continue;
    }
    const fieldPatch = diffFields(base, entry);
    if (Object.keys(fieldPatch).length > 0) {
      patch[name] = fieldPatch;
      touched = true;
    }
  }

  for (const name of baseByName.keys()) {
    if (!seen.has(name)) {
      patch[name] = null;
      touched = true;
    }
  }

  return { patch, touched };
}

/**
 * Diff two `ConfigFields` snapshots into the JSON patch shape
 * `PATCH /api/config/patch` expects — only fields that actually changed.
 */
export function diffConfigFields(
  baseline: ConfigFields,
  current: ConfigFields,
): Record<string, unknown> {
  const patch: Record<string, unknown> = {};

  for (const spec of CONFIG_FIELD_MAP) {
    const before = canonField(spec, baseline[spec.key]);
    const after = canonField(spec, current[spec.key]);
    if (!jsonEqual(before, after)) setPath(patch, spec.path, after);
  }

  const { patch: webhooksPatch, touched } = diffNamedCollection(
    baseline.webhooks,
    current.webhooks,
    webhookToJson,
    diffWebhookFields,
  );
  if (touched) patch.webhooks = webhooksPatch;

  return patch;
}

// ── Model role assignments (providers.toml `[models]` / `[background.models]`) ──

/**
 * Build the JSON patch value for a model-role assignment: a plain string
 * when there's no override, or `{"$inline": {...}}` when `temperature` or
 * `thinking` overrides the role's default. `null` clears the role.
 */
export function modelRoleJson(modelValue: string, overrides?: RoleOverrides): unknown {
  if (!modelValue) return null;
  const hasTemp = Boolean(overrides?.temperature);
  const hasThinking = Boolean(overrides?.thinking);
  if (!hasTemp && !hasThinking) return modelValue;

  const inline: Record<string, unknown> = { model: modelValue };
  if (hasTemp) inline.temperature = numberLiteral(overrides?.temperature ?? "");
  if (hasThinking) inline.thinking = overrides?.thinking;
  return { $inline: inline };
}

const MODEL_ROLE_MAP: readonly { formKey: ModelRoleKey; path: readonly string[] }[] = [
  { formKey: "main", path: ["models", "main"] },
  { formKey: "default", path: ["models", "default"] },
  { formKey: "observer", path: ["models", "observer"] },
  { formKey: "reflector", path: ["models", "reflector"] },
  { formKey: "pulse", path: ["models", "pulse"] },
  { formKey: "subconscious", path: ["models", "subconscious"] },
  { formKey: "bgSmall", path: ["background", "models", "small"] },
  { formKey: "bgMedium", path: ["background", "models", "medium"] },
  { formKey: "bgLarge", path: ["background", "models", "large"] },
];

function diffModels(
  patch: Record<string, unknown>,
  baseline: SettingsModelAssignments,
  current: SettingsModelAssignments,
): void {
  for (const { formKey, path } of MODEL_ROLE_MAP) {
    const before = modelRoleJson(baseline[formKey], baseline.overrides[formKey]);
    const after = modelRoleJson(current[formKey], current.overrides[formKey]);
    if (!jsonEqual(before, after)) setPath(patch, path, after);
  }

  const embeddingBefore = baseline.embedding || null;
  const embeddingAfter = current.embedding || null;
  if (!jsonEqual(embeddingBefore, embeddingAfter)) {
    setPath(patch, ["models", "embedding"], embeddingAfter);
  }
}

function providerToJson(p: SettingsProviderEntry): Record<string, unknown> {
  const out: Record<string, unknown> = { type: p.type };
  if (p.apiKey) out.api_key = p.apiKey;
  if (p.url) out.url = p.url;
  if (p.keepAlive) out.keep_alive = p.keepAlive;
  return out;
}

function diffProviderFields(
  base: SettingsProviderEntry,
  current: SettingsProviderEntry,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (base.type !== current.type) out.type = current.type;

  const apiKeyBefore = base.apiKey || null;
  const apiKeyAfter = current.apiKey || null;
  if (apiKeyBefore !== apiKeyAfter) out.api_key = apiKeyAfter;

  const urlBefore = base.url || null;
  const urlAfter = current.url || null;
  if (urlBefore !== urlAfter) out.url = urlAfter;

  const keepAliveBefore = base.keepAlive || null;
  const keepAliveAfter = current.keepAlive || null;
  if (keepAliveBefore !== keepAliveAfter) out.keep_alive = keepAliveAfter;

  return out;
}

/**
 * Diff providers + model role assignments into the JSON patch shape
 * `PATCH /api/providers/patch` expects.
 */
export function diffProviders(
  baselineProviders: readonly SettingsProviderEntry[],
  currentProviders: readonly SettingsProviderEntry[],
  baselineModels: SettingsModelAssignments,
  currentModels: SettingsModelAssignments,
): Record<string, unknown> {
  const patch: Record<string, unknown> = {};

  const { patch: providersPatch, touched } = diffNamedCollection(
    baselineProviders,
    currentProviders,
    providerToJson,
    diffProviderFields,
  );
  if (touched) patch.providers = providersPatch;

  diffModels(patch, baselineModels, currentModels);
  return patch;
}

// ── MCP servers (mcp.json) ───────────────────────────────────────────

function mcpServerToJson(srv: McpServerEntry): Record<string, unknown> {
  const transport = srv.transport ?? "stdio";
  const out: Record<string, unknown> = { type: transport };
  if (transport === "http") {
    out.url = srv.url || srv.command || "";
    const headers = srv.headers ?? {};
    if (Object.keys(headers).length > 0) out.headers = headers;
  } else {
    out.command = srv.command;
    if (srv.args.length > 0) out.args = srv.args;
    if (Object.keys(srv.env).length > 0) out.env = srv.env;
  }
  return out;
}

function diffMcpServerFields(
  base: McpServerEntry,
  current: McpServerEntry,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};

  const baseTransport = base.transport ?? "stdio";
  const curTransport = current.transport ?? "stdio";
  if (baseTransport !== curTransport) out.type = curTransport;

  const cmdBefore = base.command || null;
  const cmdAfter = current.command || null;
  if (cmdBefore !== cmdAfter) out.command = cmdAfter;

  if (!jsonEqual(base.args, current.args)) {
    out.args = current.args.length > 0 ? current.args : null;
  }

  if (!jsonEqual(base.env, current.env)) {
    out.env = Object.keys(current.env).length > 0 ? current.env : null;
  }

  const urlBefore = base.url || null;
  const urlAfter = current.url || null;
  if (urlBefore !== urlAfter) out.url = urlAfter;

  const baseHeaders = base.headers ?? {};
  const currentHeaders = current.headers ?? {};
  if (!jsonEqual(baseHeaders, currentHeaders)) {
    out.headers = Object.keys(currentHeaders).length > 0 ? currentHeaders : null;
  }

  return out;
}

/**
 * Diff MCP server entries into the JSON patch shape
 * `PATCH /api/mcp/patch` expects: `{"mcpServers": {...}}`.
 */
export function diffMcpServers(
  baseline: readonly McpServerEntry[],
  current: readonly McpServerEntry[],
): Record<string, unknown> {
  const { patch: serversPatch, touched } = diffNamedCollection(
    baseline,
    current,
    mcpServerToJson,
    diffMcpServerFields,
  );
  return touched ? { mcpServers: serversPatch } : {};
}
