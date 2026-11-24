export const ALL_SCOPE = "_all";

export type ScopeKind = "agent" | "all";

export type AgentSectionId =
  | "model"
  | "connections"
  | "tools"
  | "auto_mode"
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
  | "system_one"
  | "updates"
  | "limits"
  | "pricing"
  | "listener"
  | "diagnostics"
  | "raw"
  | "history";

export type SectionId = AgentSectionId | AllSectionId;

export type SectionGroup = "main" | "advanced";

export interface SectionEntry<Id extends SectionId = SectionId> {
  id: Id;
  label: string;
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

export const AGENT_SECTIONS: readonly SectionEntry<AgentSectionId>[] = [
  listed("model", "Model", "Which model it thinks with"),
  listed("connections", "Connections", "Discord, Telegram and other places to talk"),
  listed("tools", "Tools & skills", "What it can use and do"),
  listed("auto_mode", "Auto Mode", "Rules its tool calls are checked against"),
  listed("memory", "Memory", "What it keeps and when it summarizes"),
  listed("schedule", "Schedule", "Regular checks and background sessions"),
  listed("runtime", "Runtime", "Time limits and reply length", "advanced"),
  listed("servers", "Tool servers", "Outside tool servers it connects to", "advanced"),
  listed("a2a", "Agent-to-agent", "Let other agents find it and hand it work", "advanced"),
  listed("raw", "Raw config", "Edit its settings files directly", "advanced"),
  listed("history", "History", "Earlier versions of its files", "advanced"),
];

export const ALL_SECTIONS: readonly SectionEntry<AllSectionId>[] = [
  listed("general", "General", "Your timezone and where the app listens"),
  listed("notifications", "Notifications", "Alerts on this device"),
  listed("cloud", "Residuum Cloud", "Reach your agents from anywhere"),
  listed("keys", "Saved keys", "Keys and passwords your agents use"),
  listed("system_one", "Decision model", "The fast model behind Auto Mode"),
  listed("updates", "Updates", "Get the latest version"),
  listed("limits", "Session limits", "How much background work can run at once"),
  listed("pricing", "Model prices", "What estimated costs assume for each model"),
  listed("listener", "Agent-to-agent", "Let agents elsewhere hand work to yours", "advanced"),
  listed("diagnostics", "Diagnostics", "Logs and bug reports", "advanced"),
  listed("raw", "Raw config", "Edit the install-wide settings file directly", "advanced"),
  listed("history", "History", "Earlier versions of shared files", "advanced"),
];

export function scopeKind(scope: string): ScopeKind {
  return scope === ALL_SCOPE ? "all" : "agent";
}

export function sectionsOf(kind: ScopeKind): readonly SectionEntry[] {
  return kind === "all" ? ALL_SECTIONS : AGENT_SECTIONS;
}

export function defaultSection(kind: ScopeKind): SectionId {
  return kind === "all" ? "general" : "model";
}

export function isSectionOf(kind: ScopeKind, value: string): value is SectionId {
  return sectionsOf(kind).some((entry) => entry.id === value);
}

export function sectionGroups(kind: ScopeKind): Record<SectionGroup, readonly SectionEntry[]> {
  const entries = sectionsOf(kind);
  return {
    main: entries.filter((entry) => entry.group === "main"),
    advanced: entries.filter((entry) => entry.group === "advanced"),
  };
}

export function sectionAfterScopeSwitch(
  section: SectionId | null,
  to: ScopeKind,
): SectionId | null {
  if (section === null) return null;
  return isSectionOf(to, section) ? section : defaultSection(to);
}

export interface SectionTarget {
  scope: ScopeKind;
  section: SectionId;
}

export type OldSettingsScope = "agent" | "hub";

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

export function sectionFromOldName(old: string, urlScope: OldSettingsScope): SectionTarget | null {
  const agent = Object.hasOwn(OLD_AGENT_SECTIONS, old) ? OLD_AGENT_SECTIONS[old] : undefined;
  const hub = Object.hasOwn(OLD_HUB_SECTIONS, old) ? OLD_HUB_SECTIONS[old] : undefined;
  const fromAgent: SectionTarget | null =
    agent === undefined ? null : { scope: "agent", section: agent };
  const fromHub: SectionTarget | null = hub === undefined ? null : { scope: "all", section: hub };
  return urlScope === "hub" ? (fromHub ?? fromAgent) : (fromAgent ?? fromHub);
}

export type ConfigFileKind = "config" | "providers" | "mcp";

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
  auto_mode: "auto_mode",
};

const AGENT_PROVIDER_KEYS: Readonly<Record<string, AgentSectionId>> = {
  providers: "model",
  models: "model",
  background: "model",
};

const HUB_CONFIG_KEYS: Readonly<Record<string, AllSectionId>> = {
  timezone: "general",
  gateway: "general",
  push: "notifications",
  cloud: "cloud",
  background: "limits",
  pricing: "pricing",
  a2a: "listener",
  tracing: "diagnostics",
  system_one: "system_one",
};

export const RAW_SECTION: SectionId = "raw";

const SECTION_FILES: Partial<Record<`${ScopeKind}:${SectionId}`, readonly ConfigFileKind[]>> = {
  "agent:model": ["providers", "config"],
  "agent:servers": ["mcp"],
  "agent:raw": [],
  "agent:history": [],
  "all:keys": [],
  "all:updates": [],
  "all:raw": [],
  "all:history": [],
};

export function sectionFiles(kind: ScopeKind, section: SectionId): readonly ConfigFileKind[] {
  return SECTION_FILES[`${kind}:${section}`] ?? ["config"];
}

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
