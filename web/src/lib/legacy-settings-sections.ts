// The sections of the current Settings page, which still hosts every
// section's content. Hub settings apply to the whole install; agent settings
// belong to one agent. `settings-sections.ts` is the registry the URL and the
// new Settings modal use, and maps its ids onto these (`legacyHostSection`).

export type LegacyHubSection =
  | "general"
  | "cloud"
  | "a2a"
  | "sessions"
  | "tracing"
  | "update"
  | "secrets"
  | "agent-keys"
  | "history";

export type LegacyAgentSection =
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

export type LegacySection = LegacyHubSection | LegacyAgentSection;

export type LegacyScope = "agent" | "hub";

export interface LegacySectionEntry {
  id: LegacySection;
  label: string;
}

export const LEGACY_HUB_SECTIONS: readonly LegacySectionEntry[] = [
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

export const LEGACY_AGENT_SECTIONS: readonly LegacySectionEntry[] = [
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

export function legacySectionsFor(scope: LegacyScope): readonly LegacySectionEntry[] {
  return scope === "hub" ? LEGACY_HUB_SECTIONS : LEGACY_AGENT_SECTIONS;
}
