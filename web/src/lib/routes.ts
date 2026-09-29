// URL <-> app location. The chat side and the full-page places (settings,
// the workbench) are tracked separately so that leaving one returns to the
// chat side exactly as it was (same session, workspace open or not).
//
// Agent pages live under the agent that owns them; team pages are shared by
// every agent:
//
//   /agent/:name                        the agent's main chat
//   /agent/:name/sessions/:runId        a session's run in the main pane
//   /agent/:name/workspace              workspace panel open beside the chat
//   /agent/:name/sessions/:runId?workspace   the panel open beside a session
//   /agent/:name/scheduled              pulses and scheduled actions
//   /agent/:name/settings/:section      agent settings (bare /settings opens the first section)
//   /team                               the team overview: every agent, with lifecycle controls
//   /team/files                         the shared team files
//   /team/workbench                     the workbench's artifact list
//   /team/workbench/:artifact           one workbench artifact
//   /team/workbench/:artifact?full      the artifact filling the window, no Residuum chrome
//   /team/settings/:section             hub settings
//
// `/` has no agent of its own: it goes to the last-used agent. The older
// unprefixed paths (`/settings/...`, `/workbench/...`, `/scheduled`,
// `/sessions/:runId`) still resolve, to the same places under an agent or
// the team.

import { agentNameProblem } from "./agent-name";
import type { SettingsSection } from "./types";

// A record, so adding a section to `SettingsSection` fails to compile until
// it is routable too.
const SETTINGS_SECTIONS: Record<SettingsSection, true> = {
  runtime: true,
  providers: true,
  memory: true,
  integrations: true,
  mcp: true,
  "agent-keys": true,
  a2a: true,
  history: true,
};

const DEFAULT_SECTION: SettingsSection = "runtime";

/** What the chat side of the app shows. */
export interface ChatLocation {
  /** The run shown in the main pane, or null for the main chat. */
  runId: string | null;
  workspace: boolean;
}

/** What the workbench shows. */
export interface WorkbenchLocation {
  /** The artifact shown, or null for the artifact list. */
  artifact: string | null;
  /** The artifact fills the window with the Residuum UI hidden. Only with an artifact. */
  full: boolean;
}

/** Whose settings a settings page edits: one agent's, or the hub's. */
export type SettingsScope = "agent" | "hub";

export interface SettingsLocation {
  scope: SettingsScope;
  section: SettingsSection;
}

/** The team pages that are not the workbench or hub settings. */
export type TeamPage = "overview" | "files";

export interface AppLocation {
  /**
   * The agent in context. Agent pages belong to it; team pages keep it so the
   * agent's connection stays up. `null` only while no agent could be chosen.
   */
  agent: string | null;
  chat: ChatLocation;
  /** The settings page shown, or null when not on a settings page. */
  settings: SettingsLocation | null;
  /** The workbench place shown, or null when not on the workbench. */
  workbench: WorkbenchLocation | null;
  /** Whether the Scheduled view (pulses and scheduled actions) is shown. */
  scheduled: boolean;
  /** The team page shown, or null when not on one. */
  team: TeamPage | null;
}

export interface ParsedLocation {
  location: AppLocation;
  /**
   * The URL didn't name a known place (an unknown path or settings section, a
   * legacy path, or `/`), so the address bar should be corrected to the
   * formatted location.
   */
  corrected: boolean;
}

/** What a location is read against: where the user already is, and the agent to fall back on. */
export interface LocationContext {
  agent: string | null;
  chat: ChatLocation;
  /** The last-used agent, for URLs that name none. */
  fallbackAgent: string | null;
}

export const MAIN_CHAT: ChatLocation = { runId: null, workspace: false };

/** Mirrors the gateway's artifact-name rule, so a bad URL corrects to the list. */
const ARTIFACT_NAME = /^[a-z0-9]+(-[a-z0-9]+)*$/;

export function isArtifactName(value: string): boolean {
  return value.length <= 64 && ARTIFACT_NAME.test(value);
}

function isSettingsSection(value: string): value is SettingsSection {
  return Object.hasOwn(SETTINGS_SECTIONS, value);
}

function decodeSegment(segment: string): string | null {
  try {
    return decodeURIComponent(segment);
  } catch {
    return null;
  }
}

function blank(agent: string | null, chat: ChatLocation): AppLocation {
  return { agent, chat, settings: null, workbench: null, scheduled: false, team: null };
}

/** The settings page for `segment`, and whether the URL needed correcting to reach it. */
function parseSettings(
  scope: SettingsScope,
  segment: string | undefined,
): { settings: SettingsLocation; corrected: boolean } {
  const section = segment === undefined ? null : decodeSegment(segment);
  if (section !== null && isSettingsSection(section)) {
    return { settings: { scope, section }, corrected: false };
  }
  return { settings: { scope, section: DEFAULT_SECTION }, corrected: true };
}

function parseWorkbench(
  segment: string | undefined,
  search: string,
): { workbench: WorkbenchLocation; corrected: boolean } {
  const artifact = segment === undefined ? null : decodeSegment(segment);
  const valid = segment === undefined || (artifact !== null && isArtifactName(artifact));
  const shown = valid ? artifact : null;
  const wantsFull = new URLSearchParams(search).has("full");
  const full = wantsFull && shown !== null;
  return { workbench: { artifact: shown, full }, corrected: !valid || wantsFull !== full };
}

/** Whether `name` is a well-formed agent name (whether or not such an agent exists). */
export function isAgentName(name: string): boolean {
  return agentNameProblem(name) === null;
}

/**
 * Read a URL into a location. A team page says nothing about which agent or
 * chat side is open, so `context` carries those over unchanged.
 */
export function parseLocation(
  pathname: string,
  search: string,
  context: LocationContext,
): ParsedLocation {
  const segments = pathname.split("/").filter((s) => s !== "");
  const workspace = new URLSearchParams(search).has("workspace");
  const [first, second, ...rest] = segments;
  const agent = context.agent ?? context.fallbackAgent;

  if (first === "agent") {
    return parseAgentPath(second, rest, search, workspace, context);
  }
  if (first === "team") {
    return parseTeamPath(second, rest, search, context, agent);
  }

  // The bare root and the older unprefixed paths: the same places under the
  // agent in context, so the address bar is rewritten to say which.
  const legacy = parseLegacyPath(first, second, rest, search, workspace, context, agent);
  return (
    legacy ?? {
      location: blank(agent, MAIN_CHAT),
      corrected: first !== undefined || agent !== null,
    }
  );
}

function parseAgentPath(
  name: string | undefined,
  rest: string[],
  search: string,
  workspace: boolean,
  context: LocationContext,
): ParsedLocation {
  const agent = name === undefined ? null : decodeSegment(name);
  if (agent === null || !isAgentName(agent)) {
    const fallback = context.agent ?? context.fallbackAgent;
    return { location: blank(fallback, MAIN_CHAT), corrected: true };
  }
  const [page, arg, ...extra] = rest;
  // Another agent's open session is not this agent's to return to.
  const carried = agent === context.agent ? context.chat : MAIN_CHAT;

  if (page === undefined) {
    return { location: blank(agent, { runId: null, workspace }), corrected: false };
  }
  if (page === "workspace" && arg === undefined) {
    return { location: blank(agent, { runId: null, workspace: true }), corrected: false };
  }
  if (page === "scheduled" && arg === undefined) {
    return { location: { ...blank(agent, carried), scheduled: true }, corrected: false };
  }
  if (page === "settings" && extra.length === 0) {
    const { settings, corrected } = parseSettings("agent", arg);
    return { location: { ...blank(agent, carried), settings }, corrected };
  }
  if (page === "sessions" && arg !== undefined && extra.length === 0) {
    const runId = decodeSegment(arg);
    if (runId !== null && runId !== "") {
      return { location: blank(agent, { runId, workspace }), corrected: false };
    }
  }
  return { location: blank(agent, { runId: null, workspace }), corrected: true };
}

function parseTeamPath(
  page: string | undefined,
  rest: string[],
  search: string,
  context: LocationContext,
  agent: string | null,
): ParsedLocation {
  const [arg, ...extra] = rest;
  const base = blank(agent, context.chat);

  if (page === undefined) {
    return { location: { ...base, team: "overview" }, corrected: false };
  }
  if (page === "files" && arg === undefined) {
    return { location: { ...base, team: "files" }, corrected: false };
  }
  if (page === "workbench" && extra.length === 0) {
    const { workbench, corrected } = parseWorkbench(arg, search);
    return { location: { ...base, workbench }, corrected };
  }
  if (page === "settings" && extra.length === 0) {
    const { settings, corrected } = parseSettings("hub", arg);
    return { location: { ...base, settings }, corrected };
  }
  return { location: { ...base, team: "overview" }, corrected: true };
}

function parseLegacyPath(
  first: string | undefined,
  second: string | undefined,
  rest: string[],
  search: string,
  workspace: boolean,
  context: LocationContext,
  agent: string | null,
): ParsedLocation | null {
  const base = blank(agent, context.chat);
  if (first === "settings" && rest.length === 0) {
    const { settings } = parseSettings("agent", second);
    return { location: { ...base, settings }, corrected: true };
  }
  if (first === "workbench" && rest.length === 0) {
    const { workbench } = parseWorkbench(second, search);
    return { location: { ...base, workbench }, corrected: true };
  }
  if (first === "scheduled" && rest.length === 0 && second === undefined) {
    return { location: { ...base, scheduled: true }, corrected: true };
  }
  if (first === "sessions" && second !== undefined && rest.length === 0) {
    const runId = decodeSegment(second);
    if (runId !== null && runId !== "") {
      return { location: blank(agent, { runId, workspace }), corrected: true };
    }
  }
  return null;
}

/** The URL (path and query) for a location. */
export function formatLocation(location: AppLocation): string {
  const { agent } = location;
  if (location.settings !== null) {
    const { scope, section } = location.settings;
    if (scope === "hub") return `/team/settings/${section}`;
    return agent === null ? "/" : `/agent/${encodeURIComponent(agent)}/settings/${section}`;
  }
  if (location.workbench !== null) {
    const { artifact, full } = location.workbench;
    if (artifact === null) return "/team/workbench";
    return full ? `/team/workbench/${artifact}?full` : `/team/workbench/${artifact}`;
  }
  if (location.team === "overview") return "/team";
  if (location.team === "files") return "/team/files";
  if (agent === null) return "/";
  const base = `/agent/${encodeURIComponent(agent)}`;
  if (location.scheduled) return `${base}/scheduled`;
  const { runId, workspace } = location.chat;
  if (runId === null) return workspace ? `${base}/workspace` : base;
  const path = `${base}/sessions/${encodeURIComponent(runId)}`;
  return workspace ? `${path}?workspace` : path;
}
