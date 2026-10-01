// The settings section registry (design §8). Every setting belongs to one
// scope: "All agents" (install-wide, the hub's config) or one agent (that
// agent's config files). The URL names a scope and a section
// (`?settings=scout/model`, `?settings=_all/general`); this module holds what
// those names mean: the ids, labels and groups, where each old section name
// went, and which section a config key belongs to.

/** The URL token for the install-wide scope. Agent names can't contain an underscore, so it can't collide with one. */
export const ALL_SCOPE = "_all";

/** Which kind of scope: one agent's settings, or the install's. */
export type ScopeKind = "agent" | "all";

export type AgentSectionId =
  | "model"
  | "connections"
  | "tools"
  | "memory"
  | "schedule"
  | "runtime"
  | "servers"
  | "a2a"
  | "raw"
  | "history";

export type AllSectionId =
  | "general"
  | "notifications"
  | "cloud"
  | "keys"
  | "updates"
  | "limits"
  | "listener"
  | "diagnostics"
  | "raw"
  | "history";

export type SectionId = AgentSectionId | AllSectionId;

/** The Advanced group is shown under a non-interactive heading. */
export type SectionGroup = "main" | "advanced";

export interface SectionEntry<Id extends SectionId = SectionId> {
  id: Id;
  label: string;
  /** What it holds, in a few words. The phone's section list shows it under the label. */
  description: string;
  group: SectionGroup;
}

function listed<Id extends SectionId>(
  id: Id,
  label: string,
  description: string,
  group: SectionGroup = "main",
): SectionEntry<Id> {
  return { id, label, description, group };
}

/** An agent's sections, in the order the list shows them. */
export const AGENT_SECTIONS: readonly SectionEntry<AgentSectionId>[] = [
  listed("model", "Model", "Which model it thinks with"),
  listed("connections", "Connections", "Discord, Telegram and other places to talk"),
  listed("tools", "Tools & skills", "What it can use and do"),
  listed("memory", "Memory", "What it keeps and when it summarizes"),
  listed("schedule", "Schedule", "Regular checks and background sessions"),
  listed("runtime", "Runtime", "Time limits and reply length", "advanced"),
  listed("servers", "Tool servers", "Outside tool servers it connects to", "advanced"),
  listed("a2a", "Agent-to-agent", "Let other agents find it and hand it work", "advanced"),
  listed("raw", "Raw config", "Edit its settings files directly", "advanced"),
  listed("history", "History", "Earlier versions of its files", "advanced"),
];

/** The install-wide sections, in the order the list shows them. */
export const ALL_SECTIONS: readonly SectionEntry<AllSectionId>[] = [
  listed("general", "General", "Your timezone and where the app listens"),
  listed("notifications", "Notifications", "Alerts on this device"),
  listed("cloud", "Residuum Cloud", "Reach your agents from anywhere"),
  listed("keys", "Saved keys", "Keys and passwords your agents use"),
  listed("updates", "Updates", "Get the latest version"),
  listed("limits", "Session limits", "How much background work can run at once"),
  listed("listener", "Agent-to-agent", "Let agents elsewhere hand work to yours", "advanced"),
  listed("diagnostics", "Diagnostics", "Logs and bug reports", "advanced"),
  listed("raw", "Raw config", "Edit the install-wide settings file directly", "advanced"),
  listed("history", "History", "Earlier versions of shared files", "advanced"),
];

/** What a scope token in the URL means: `_all` is the install, anything else an agent's name. */
export function scopeKind(scope: string): ScopeKind {
  return scope === ALL_SCOPE ? "all" : "agent";
}

export function sectionsOf(kind: ScopeKind): readonly SectionEntry[] {
  return kind === "all" ? ALL_SECTIONS : AGENT_SECTIONS;
}

/** The section a scope opens on when the URL names none (on phones the list opens instead). */
export function defaultSection(kind: ScopeKind): SectionId {
  return kind === "all" ? "general" : "model";
}

export function isSectionOf(kind: ScopeKind, value: string): value is SectionId {
  return sectionsOf(kind).some((entry) => entry.id === value);
}

/** The scope's sections split into the main list and the Advanced group. */
export function sectionGroups(kind: ScopeKind): Record<SectionGroup, readonly SectionEntry[]> {
  const entries = sectionsOf(kind);
  return {
    main: entries.filter((entry) => entry.group === "main"),
    advanced: entries.filter((entry) => entry.group === "advanced"),
  };
}

/**
 * The section to show after switching to another scope: the current one when
 * the new scope has it (`raw` and `history` exist in both), else the new
 * scope's default. No current section (the phone's section list) stays none.
 */
export function sectionAfterScopeSwitch(
  section: SectionId | null,
  to: ScopeKind,
): SectionId | null {
  if (section === null) return null;
  return isSectionOf(to, section) ? section : defaultSection(to);
}

// ── Old section names ────────────────────────────────────────────────

export interface SectionTarget {
  scope: ScopeKind;
  section: SectionId;
}

/** Which page an old settings URL was under: an agent's settings, or the hub's. */
export type OldSettingsScope = "agent" | "hub";

/** Where each section of an old agent settings page went. */
const OLD_AGENT_SECTIONS: Readonly<Record<string, AgentSectionId>> = {
  runtime: "runtime",
  providers: "model",
  channels: "connections",
  integrations: "connections",
  webhooks: "connections",
  pulses: "schedule",
  memory: "memory",
  skills: "tools",
  mcp: "servers",
  a2a: "a2a",
  history: "history",
};

/** Where each section of an old hub settings page went. */
const OLD_HUB_SECTIONS: Readonly<Record<string, AllSectionId>> = {
  general: "general",
  cloud: "cloud",
  a2a: "listener",
  sessions: "limits",
  tracing: "diagnostics",
  update: "updates",
  secrets: "keys",
  "agent-keys": "keys",
  history: "history",
};

/**
 * Where an old section name lives now, for redirecting an old settings URL.
 * `urlScope` is the page the old URL was under (an agent's settings or the
 * hub's). A name the other old scope has moves to that scope, and the name
 * both have (`a2a`, `history`) stays with the scope the URL named. Returns
 * null for a name no old page had.
 */
export function sectionFromOldName(old: string, urlScope: OldSettingsScope): SectionTarget | null {
  const agent = Object.hasOwn(OLD_AGENT_SECTIONS, old) ? OLD_AGENT_SECTIONS[old] : undefined;
  const hub = Object.hasOwn(OLD_HUB_SECTIONS, old) ? OLD_HUB_SECTIONS[old] : undefined;
  const fromAgent: SectionTarget | null =
    agent === undefined ? null : { scope: "agent", section: agent };
  const fromHub: SectionTarget | null = hub === undefined ? null : { scope: "all", section: hub };
  return urlScope === "hub" ? (fromHub ?? fromAgent) : (fromAgent ?? fromHub);
}

// ── Config keys ──────────────────────────────────────────────────────

/** The config files a scope's forms edit. The hub has only `config`. */
export type ConfigFileKind = "config" | "providers" | "mcp";

/** An agent's `config.toml`, by top-level key. `autostart` is an immediate action, so it has no section. */
const AGENT_CONFIG_KEYS: Readonly<Record<string, AgentSectionId>> = {
  temperature: "model",
  thinking: "model",
  discord: "connections",
  telegram: "connections",
  teams: "connections",
  webhooks: "connections",
  skills: "tools",
  tools: "tools",
  web_search: "tools",
  memory: "memory",
  subconscious: "memory",
  learning: "memory",
  pulse: "schedule",
  background: "schedule",
  timeout_secs: "runtime",
  max_tokens: "runtime",
  retry: "runtime",
  agent: "runtime",
  idle: "runtime",
  a2a: "a2a",
};

/** An agent's `providers.toml`, by top-level key. */
const AGENT_PROVIDER_KEYS: Readonly<Record<string, AgentSectionId>> = {
  providers: "model",
  models: "model",
  background: "model",
};

/** The hub's `config.toml`, by top-level key. */
const HUB_CONFIG_KEYS: Readonly<Record<string, AllSectionId>> = {
  timezone: "general",
  gateway: "general",
  push: "notifications",
  cloud: "cloud",
  background: "limits",
  a2a: "listener",
  tracing: "diagnostics",
};

/** The section that holds a config file's raw editor. */
export const RAW_SECTION: SectionId = "raw";

/**
 * The section whose form edits a config key, by its top-level key, so a
 * diagnostic on `memory.observer_threshold_tokens` leads to Memory. Null when
 * no form edits the key, which sends a diagnostic to Raw config. This is a
 * coarse table; the settings model's field map is the exact one.
 */
export function sectionForConfigKey(
  kind: ScopeKind,
  file: ConfigFileKind,
  keyPath: string,
): SectionId | null {
  const top = keyPath.split(/[.[]/, 1)[0] ?? "";
  if (top === "") return null;
  if (kind === "all") {
    return file === "config" && Object.hasOwn(HUB_CONFIG_KEYS, top)
      ? (HUB_CONFIG_KEYS[top] ?? null)
      : null;
  }
  if (file === "mcp") return "servers";
  const table = file === "providers" ? AGENT_PROVIDER_KEYS : AGENT_CONFIG_KEYS;
  return Object.hasOwn(table, top) ? (table[top] ?? null) : null;
}
