import { isAgentSlug } from "./agent-name";
import type { MailFolder } from "./hub-types";
import {
  ALL_SCOPE,
  defaultSection,
  isSectionOf,
  scopeKind,
  sectionFromOldName,
  type OldSettingsScope,
  type ScopeKind,
  type SectionId,
} from "./settings-sections";

export const AGENT_PANEL_KINDS = ["activity", "schedule", "files"] as const;

export type AgentPanelKind = (typeof AGENT_PANEL_KINDS)[number];

export type AgentPlaceKind = "chat" | AgentPanelKind;

export function isAgentPanelKind(kind: string): kind is AgentPanelKind {
  return (AGENT_PANEL_KINDS as readonly string[]).includes(kind);
}

export type AgentPlaceWhere = "page" | "beside-chat";

export type { MailFolder };

export interface ComposeDraft {
  to: string[];
}

export type Place =
  | { kind: "home" }
  | {
      kind: "inbox";
      folder: MailFolder;
      agent: string | null;
      thread: string | null;
      compose: ComposeDraft | null;
    }
  | { kind: AgentPlaceKind; agent: string }
  | { kind: "workbench"; artifact: string | null }
  | { kind: "shared-files" }
  | { kind: "not-found"; path: string };

export type AgentPlace = Extract<Place, { kind: AgentPlaceKind }>;

export type InboxPlace = Extract<Place, { kind: "inbox" }>;

export type Panel =
  | { kind: "session"; agent: string; runId: string }
  | { kind: "new-session"; agent: string }
  | { kind: AgentPanelKind; agent: string }
  | { kind: "file"; path: string; agent: string | null }
  | { kind: "size" };

export interface SettingsTarget {
  scope: string;
  section: SectionId | null;
}

export interface AppLocation {
  place: Place;
  panel: Panel | null;
  settings: SettingsTarget | null;
}

export const HOME: Place = { kind: "home" };

export const INBOX: InboxPlace = {
  kind: "inbox",
  folder: "inbox",
  agent: null,
  thread: null,
  compose: null,
};

export function userInboxRef(agent: string, id: string): string {
  return `${agent}/user/${id}`;
}

export function locationAt(place: Place): AppLocation {
  return { place, panel: null, settings: null };
}

export function isAgentPlace(place: Place): place is AgentPlace {
  return place.kind === "chat" || isAgentPanelKind(place.kind);
}

export function agentPlaceLocation(
  agent: string,
  kind: AgentPlaceKind,
  where: AgentPlaceWhere,
): AppLocation {
  if (kind === "chat" || where === "page") return locationAt({ kind, agent });
  return { place: { kind: "chat", agent }, panel: { kind, agent }, settings: null };
}

export function showsAgentPlace(
  location: AppLocation,
  agent: string,
  kind: AgentPanelKind,
): boolean {
  const { place, panel } = location;
  if (isAgentPlace(place) && place.kind === kind && place.agent === agent) return true;
  return (
    place.kind === "chat" && place.agent === agent && panel?.kind === kind && panel.agent === agent
  );
}

export function viewedAgentOf(place: Place): string | null {
  return isAgentPlace(place) ? place.agent : null;
}

const ARTIFACT_NAME = /^[a-z0-9]+(-[a-z0-9]+)*$/;

export function isArtifactName(value: string): boolean {
  return value.length <= 64 && ARTIFACT_NAME.test(value);
}

export function isAgentName(name: string): boolean {
  return isAgentSlug(name);
}

export function panelAllowed(place: Place, panel: Panel): boolean {
  if (isAgentOwnPanel(panel)) return isAgentPlace(place) && place.agent === panel.agent;
  switch (panel.kind) {
    case "session":
      return (isAgentPlace(place) && place.agent === panel.agent) || place.kind === "workbench";
    case "file":
      return (
        isAgentPlace(place) ||
        place.kind === "shared-files" ||
        (panel.agent !== null && place.kind === "inbox")
      );
    case "size":
      return isAgentPlace(place);
  }
}

type AgentOwnPanel = Extract<Panel, { kind: "new-session" | AgentPanelKind }>;

function isAgentOwnPanel(panel: Panel): panel is AgentOwnPanel {
  return panel.kind === "new-session" || isAgentPanelKind(panel.kind);
}

export interface ParseContext {
  lastUsed: string | null;
}

export interface ParsedUrl {
  location: AppLocation;
  notices: string[];
  needsLastUsed: boolean;
}

export function missingAgentNotice(name: string): string {
  return `There's no agent named "${name}".`;
}

function decodeSegment(segment: string): string | null {
  try {
    return decodeURIComponent(segment);
  } catch {
    return null;
  }
}

function parsePanel(value: string): Panel | null {
  if (value === "size") return { kind: "size" };
  const colon = value.indexOf(":");
  if (colon < 0) return null;
  const kind = value.slice(0, colon);
  const rest = value.slice(colon + 1);
  if (kind === "file") return rest === "" ? null : { kind: "file", path: rest, agent: null };
  if (kind === "agent-file") {
    const split = rest.indexOf(":");
    if (split < 0) return null;
    const agent = rest.slice(0, split);
    const path = rest.slice(split + 1);
    return isAgentName(agent) && path !== "" ? { kind: "file", path, agent } : null;
  }
  if (kind === "new-session" || isAgentPanelKind(kind))
    return isAgentName(rest) ? { kind, agent: rest } : null;
  if (kind === "session") {
    const split = rest.indexOf(":");
    if (split < 0) return null;
    const agent = rest.slice(0, split);
    const runId = rest.slice(split + 1);
    return isAgentName(agent) && runId !== "" ? { kind: "session", agent, runId } : null;
  }
  return null;
}

function parseSettings(value: string): { target: SettingsTarget; notices: string[] } | null {
  const slash = value.indexOf("/");
  const scope = slash < 0 ? value : value.slice(0, slash);
  const named = slash < 0 ? null : value.slice(slash + 1);
  if (scope === "") return null;
  if (scope !== ALL_SCOPE && !isAgentName(scope)) {
    return {
      target: { scope: ALL_SCOPE, section: "general" },
      notices: [missingAgentNotice(scope)],
    };
  }
  const kind = scopeKind(scope);
  let section: SectionId | null = null;
  if (named !== null) section = isSectionOf(kind, named) ? named : defaultSection(kind);
  return { target: { scope, section }, notices: [] };
}

const MAIL_FOLDERS: readonly MailFolder[] = ["inbox", "sent", "archived"];

function isMailFolder(value: string): value is MailFolder {
  return (MAIL_FOLDERS as readonly string[]).includes(value);
}

const REF_KINDS: ReadonlySet<string> = new Set(["user", "agent"]);

function parseThreadRef(value: string | null): string | null {
  if (value === null) return null;
  const parts = value.split("/");
  if (parts.length !== 3) return null;
  const [agent = "", kind = "", id = ""] = parts;
  return isAgentName(agent) && REF_KINDS.has(kind) && id !== "" ? value : null;
}

function refOfOldItem(value: string | null): string | null {
  const colon = value === null ? -1 : value.indexOf(":");
  if (value === null || colon <= 0) return null;
  const agent = value.slice(0, colon);
  const id = value.slice(colon + 1);
  return isAgentName(agent) && id !== "" ? userInboxRef(agent, id) : null;
}

function parseRecipients(value: string | null): string[] {
  if (value === null) return [];
  const names = value.split(",").filter(isAgentName);
  return names.filter((name, index) => names.indexOf(name) === index);
}

function parseInbox(query: URLSearchParams): Place {
  const agentParam = query.get("agent");
  const agent = agentParam !== null && isAgentName(agentParam) ? agentParam : null;
  const folderParam = query.get("folder");
  let folder: MailFolder = query.get("tab") === "archived" ? "archived" : "inbox";
  if (folderParam !== null && isMailFolder(folderParam)) folder = folderParam;
  if (query.get("compose") === "1") {
    return {
      kind: "inbox",
      folder,
      agent,
      thread: null,
      compose: { to: parseRecipients(query.get("to")) },
    };
  }
  const thread = parseThreadRef(query.get("thread")) ?? refOfOldItem(query.get("item"));
  return { kind: "inbox", folder, agent, thread, compose: null };
}

interface PathResult {
  place: Place;
  panel?: Panel;
  settings?: SettingsTarget;
  underLastUsed?: (agent: string) => PathResult;
}

function workbenchPlace(artifact: string | undefined): Place {
  return artifact !== undefined && isArtifactName(artifact)
    ? { kind: "workbench", artifact }
    : { kind: "workbench", artifact: null };
}

function oldSettingsTarget(
  urlScope: OldSettingsScope,
  agent: string | null,
  old: string | undefined,
): { target: SettingsTarget; needsAgent: boolean } {
  const found = old === undefined ? null : sectionFromOldName(old, urlScope);
  const kind: ScopeKind = found?.scope ?? (urlScope === "hub" ? "all" : "agent");
  let section: SectionId | null = null;
  if (found !== null) section = found.section;
  else if (old !== undefined) section = defaultSection(kind);
  const scope = kind === "all" ? ALL_SCOPE : (agent ?? "");
  return { target: { scope, section }, needsAgent: kind === "agent" && agent === null };
}

function parseAgentPath(name: string, rest: string[], query: URLSearchParams): PathResult | null {
  if (!isAgentName(name)) return null;
  const [page, arg, ...extra] = rest;
  const chat: Place = { kind: "chat", agent: name };
  const files: Place = { kind: "files", agent: name };
  if (page === undefined) return { place: query.has("workspace") ? files : chat };
  if (extra.length > 0) return null;
  if (arg === undefined) {
    if (page === "activity" || page === "schedule" || page === "files") {
      return { place: { kind: page, agent: name } };
    }
    if (page === "workspace") return { place: files };
    if (page === "scheduled") return { place: { kind: "schedule", agent: name } };
    if (page === "settings") return { place: chat, settings: { scope: name, section: null } };
    return null;
  }
  if (page === "sessions" && arg !== "") {
    return { place: chat, panel: { kind: "session", agent: name, runId: arg } };
  }
  if (page === "settings") {
    return { place: chat, settings: oldSettingsTarget("agent", name, arg).target };
  }
  return null;
}

function parseTeamPath(page: string | undefined, rest: string[]): PathResult | null {
  const [arg, ...extra] = rest;
  if (page === undefined) return { place: HOME };
  if (page === "files" && arg === undefined) return { place: { kind: "shared-files" } };
  if (page === "workbench" && extra.length === 0) return { place: workbenchPlace(arg) };
  if (page === "settings" && extra.length === 0) {
    const { target, needsAgent } = oldSettingsTarget("hub", null, arg);
    if (!needsAgent) return { place: HOME, settings: target };
    return {
      place: HOME,
      underLastUsed: (agent) => ({
        place: { kind: "chat", agent },
        settings: oldSettingsTarget("hub", agent, arg).target,
      }),
    };
  }
  return null;
}

function parseOldPath(segments: string[]): PathResult | null {
  const [first, second, ...rest] = segments;
  if (first === "workbench" && rest.length === 0) return { place: workbenchPlace(second) };
  if (rest.length > 0) return null;
  if (first === "settings") {
    return {
      place: HOME,
      underLastUsed: (agent) => ({
        place: { kind: "chat", agent },
        settings: oldSettingsTarget("agent", agent, second).target,
      }),
    };
  }
  if (first === "scheduled" && second === undefined) {
    return { place: HOME, underLastUsed: (agent) => ({ place: { kind: "schedule", agent } }) };
  }
  if (first === "sessions" && second !== undefined && second !== "") {
    return {
      place: HOME,
      underLastUsed: (agent) => ({
        place: { kind: "chat", agent },
        panel: { kind: "session", agent, runId: second },
      }),
    };
  }
  if (first === "notification" && second !== undefined) {
    return { place: HOME, underLastUsed: (agent) => ({ place: { kind: "files", agent } }) };
  }
  return null;
}

function parsePath(segments: string[], query: URLSearchParams): PathResult | null {
  const [first, second, ...rest] = segments;
  if (first === undefined) return { place: HOME };
  if (first === "home" && second === undefined) return { place: HOME };
  if (first === "inbox" && second === undefined) return { place: parseInbox(query) };
  if (first === "agent" && second !== undefined) return parseAgentPath(second, rest, query);
  if (first === "team") return parseTeamPath(second, rest);
  return parseOldPath(segments);
}

export function parseUrl(pathname: string, search: string, context: ParseContext): ParsedUrl {
  const query = new URLSearchParams(search);
  const segments: string[] = [];
  let readable = true;
  for (const raw of pathname.split("/")) {
    if (raw === "") continue;
    const segment = decodeSegment(raw);
    if (segment === null) readable = false;
    else segments.push(segment);
  }
  let path: PathResult = (readable ? parsePath(segments, query) : null) ?? {
    place: { kind: "not-found", path: pathname },
  };

  if (path.underLastUsed !== undefined) {
    if (context.lastUsed === null) {
      return { location: locationAt(HOME), notices: [], needsLastUsed: true };
    }
    path = path.underLastUsed(context.lastUsed);
  }

  const notices: string[] = [];
  const { place } = path;
  let panel = path.panel ?? null;
  if (panel === null) {
    const param = query.get("panel");
    panel = param === null ? null : parsePanel(param);
  }
  if (panel !== null && !panelAllowed(place, panel)) panel = null;

  let settings = path.settings ?? null;
  if (settings === null) {
    const param = query.get("settings");
    const parsed = param === null ? null : parseSettings(param);
    if (parsed !== null) {
      settings = parsed.target;
      notices.push(...parsed.notices);
    }
  }
  return { location: { place, panel, settings }, notices, needsLastUsed: false };
}

export function correctForAgents(
  location: AppLocation,
  known: ReadonlySet<string>,
): { location: AppLocation; notices: string[] } {
  const missing: string[] = [];
  const note = (name: string): void => {
    if (!known.has(name) && !missing.includes(name)) missing.push(name);
  };
  let { place, panel, settings } = location;

  if (isAgentPlace(place) && !known.has(place.agent)) {
    note(place.agent);
    place = HOME;
  } else if (place.kind === "inbox") {
    let { agent, compose, thread } = place;
    if (agent !== null && !known.has(agent)) {
      note(agent);
      agent = null;
    }
    const threadAgent = thread?.split("/")[0];
    if (threadAgent !== undefined && !known.has(threadAgent)) {
      note(threadAgent);
      thread = null;
    }
    if (compose !== null) {
      const to = compose.to.filter((name) => {
        note(name);
        return known.has(name);
      });
      if (to.length !== compose.to.length) compose = { to };
    }
    place = { ...place, agent, compose, thread };
  }
  const panelAgent = panel !== null && "agent" in panel ? panel.agent : null;
  if (panelAgent !== null && !known.has(panelAgent)) {
    note(panelAgent);
    panel = null;
  }
  if (panel !== null && !panelAllowed(place, panel)) panel = null;
  if (settings !== null && scopeKind(settings.scope) === "agent" && !known.has(settings.scope)) {
    note(settings.scope);
    settings = { scope: ALL_SCOPE, section: "general" };
  }
  return { location: { place, panel, settings }, notices: missing.map(missingAgentNotice) };
}

export function correctForArtifacts(
  location: AppLocation,
  artifacts: ReadonlySet<string>,
): { location: AppLocation; notices: string[] } {
  const { place } = location;
  if (place.kind !== "workbench" || place.artifact === null || artifacts.has(place.artifact)) {
    return { location, notices: [] };
  }
  return {
    location: { ...location, place: { kind: "workbench", artifact: null } },
    notices: [`There's no workbench page named "${place.artifact}".`],
  };
}

function queryValue(value: string): string {
  return encodeURIComponent(value).replace(/%3A/gi, ":").replace(/%2F/gi, "/");
}

function formatPanel(panel: Panel): string {
  if (isAgentOwnPanel(panel)) return `${panel.kind}:${panel.agent}`;
  switch (panel.kind) {
    case "size":
      return "size";
    case "file":
      return panel.agent === null
        ? `file:${panel.path}`
        : `agent-file:${panel.agent}:${panel.path}`;
    case "session":
      return `session:${panel.agent}:${panel.runId}`;
  }
}

function formatPlace(place: Place): { path: string; params: string[] } {
  switch (place.kind) {
    case "home":
      return { path: "/home", params: [] };
    case "inbox": {
      const params: string[] = [];
      if (place.folder !== "inbox") params.push(`folder=${place.folder}`);
      if (place.agent !== null) params.push(`agent=${queryValue(place.agent)}`);
      if (place.compose !== null) {
        params.push("compose=1");
        if (place.compose.to.length > 0) {
          params.push(`to=${place.compose.to.map((name) => encodeURIComponent(name)).join(",")}`);
        }
      } else if (place.thread !== null) {
        params.push(`thread=${encodeURIComponent(place.thread)}`);
      }
      return { path: "/inbox", params };
    }
    case "workbench": {
      const path =
        place.artifact === null
          ? "/team/workbench"
          : `/team/workbench/${encodeURIComponent(place.artifact)}`;
      return { path, params: [] };
    }
    case "shared-files":
      return { path: "/team/files", params: [] };
    case "not-found":
      return { path: place.path, params: [] };
    case "chat":
      return { path: `/agent/${encodeURIComponent(place.agent)}`, params: [] };
    case "activity":
    case "schedule":
    case "files":
      return { path: `/agent/${encodeURIComponent(place.agent)}/${place.kind}`, params: [] };
  }
}

export function formatLocation(location: AppLocation): string {
  const { path, params } = formatPlace(location.place);
  if (location.panel !== null) params.push(`panel=${queryValue(formatPanel(location.panel))}`);
  if (location.settings !== null) {
    const { scope, section } = location.settings;
    params.push(`settings=${queryValue(section === null ? scope : `${scope}/${section}`)}`);
  }
  return params.length === 0 ? path : `${path}?${params.join("&")}`;
}

export function locationsEqual(a: AppLocation, b: AppLocation): boolean {
  return formatLocation(a) === formatLocation(b);
}

export function placesEqual(a: Place, b: Place): boolean {
  return formatLocation(locationAt(a)) === formatLocation(locationAt(b));
}
