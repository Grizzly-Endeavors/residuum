// The settings pages, split by whose settings they edit. Hub settings apply to
// the whole install and live under `/team/settings`; agent settings belong to
// one agent and live under `/agent/<name>/settings`.

export type HubSettingsSection =
  | "general"
  | "cloud"
  | "a2a"
  | "sessions"
  | "tracing"
  | "update"
  | "secrets"
  | "agent-keys"
  | "history";

export type AgentSettingsSection =
  | "runtime"
  | "providers"
  | "channels"
  | "pulses"
  | "memory"
  | "skills"
  | "mcp"
  | "a2a"
  | "webhooks"
  | "history";

export type SettingsSection = HubSettingsSection | AgentSettingsSection;

export type SettingsScope = "agent" | "hub";

export interface SectionEntry {
  id: SettingsSection;
  label: string;
}

export const HUB_SECTIONS: readonly SectionEntry[] = [
  { id: "general", label: "Gateway & timezone" },
  { id: "cloud", label: "Cloud" },
  { id: "a2a", label: "A2A listener & keys" },
  { id: "sessions", label: "Session budget" },
  { id: "tracing", label: "Tracing" },
  { id: "update", label: "Update" },
  { id: "secrets", label: "Secrets" },
  { id: "agent-keys", label: "Agent keys" },
  { id: "history", label: "History" },
];

export const AGENT_SECTIONS: readonly SectionEntry[] = [
  { id: "runtime", label: "Runtime" },
  { id: "providers", label: "Models & providers" },
  { id: "channels", label: "Adapters & channels" },
  { id: "pulses", label: "Pulses & sessions" },
  { id: "memory", label: "Memory" },
  { id: "skills", label: "Skills & tools" },
  { id: "mcp", label: "MCP" },
  { id: "a2a", label: "A2A visibility & client" },
  { id: "webhooks", label: "Webhooks" },
  { id: "history", label: "History" },
];

export function sectionsFor(scope: SettingsScope): readonly SectionEntry[] {
  return scope === "hub" ? HUB_SECTIONS : AGENT_SECTIONS;
}

export function defaultSection(scope: SettingsScope): SettingsSection {
  return scope === "hub" ? "general" : "runtime";
}

export function isSectionOf(scope: SettingsScope, value: string): value is SettingsSection {
  return sectionsFor(scope).some((s) => s.id === value);
}

/** Older section names that moved to another scope or were renamed. */
const MOVED_SECTIONS: Readonly<Record<string, { scope: SettingsScope; section: SettingsSection }>> =
  {
    "agent-keys": { scope: "hub", section: "agent-keys" },
    integrations: { scope: "agent", section: "channels" },
  };

/**
 * Find where a section name lives when the URL may have named the wrong
 * scope (an old `/settings/agent-keys`, or a hub link naming an agent
 * section). The preferred scope wins when both have the section. Returns
 * null for an unknown name.
 */
export function locateSection(
  value: string,
  preferred: SettingsScope,
): { scope: SettingsScope; section: SettingsSection } | null {
  const moved = MOVED_SECTIONS[value];
  if (moved) return moved;
  const other: SettingsScope = preferred === "hub" ? "agent" : "hub";
  for (const scope of [preferred, other]) {
    if (isSectionOf(scope, value)) return { scope, section: value };
  }
  return null;
}
