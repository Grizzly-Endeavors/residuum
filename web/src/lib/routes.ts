// URL <-> app location. A location is a place (a destination in
// the main region), an optional context panel, and an optional Settings modal:
//
//   /home                          Home
//   /inbox                         Inbox (?folder=sent|archived &agent=<name> &thread=<ref>
//                                  or &compose=1 &to=<name>,<name>)
//   /agent/:name                   an agent's Chat
//   /agent/:name/activity          its Activity
//   /agent/:name/schedule          its Schedule
//   /agent/:name/files             its Files
//   /team/workbench[/:artifact]    the Workbench list, with an artifact's row selected
//   /team/files                    Shared files
//
//   ?panel=session:<agent>:<runId> | new-session:<agent> | file:<path>
//          | agent-file:<agent>:<path> | size               the context panel
//   ?settings=<agent | _all>[/<section>]                    the Settings modal
//
// Paths that older versions used redirect to these, and any other path is the
// Not Found place, which keeps the address as it was typed. Nothing here
// touches the browser: the router applies what this module reads and formats.

import { isAgentSlug } from "./agent-name";
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

export type AgentPlaceKind = "chat" | "activity" | "schedule" | "files";

/** The Inbox's folders: what arrived, what the user sent, and what was set aside. */
export type MailFolder = "inbox" | "sent" | "archived";

/** A New message that is open: the agents it is addressed to so far. */
export interface ComposeDraft {
  to: string[];
}

export type Place =
  | { kind: "home" }
  | {
      kind: "inbox";
      folder: MailFolder;
      /** Shows only the threads this agent is in; null shows every agent's. */
      agent: string | null;
      /** The ref of a message in the open thread (the thread's own id, or any message's); null when none is open. */
      thread: string | null;
      /** The New message that is open. It takes the thread's place when both are set. */
      compose: ComposeDraft | null;
    }
  | { kind: AgentPlaceKind; agent: string }
  | { kind: "workbench"; artifact: string | null }
  | { kind: "shared-files" }
  /** An address that leads nowhere in the app. `path` is the address's path as it arrived, still encoded. */
  | { kind: "not-found"; path: string };

export type AgentPlace = Extract<Place, { kind: AgentPlaceKind }>;

export type InboxPlace = Extract<Place, { kind: "inbox" }>;

export type Panel =
  | { kind: "session"; agent: string; runId: string }
  | { kind: "new-session"; agent: string }
  /**
   * A file. With no `agent` it is read from the tree the place shows (an
   * agent's workspace on its places, the team's folder on Shared files); with
   * one it is that agent's workspace, whatever place the panel is beside.
   */
  | { kind: "file"; path: string; agent: string | null }
  | { kind: "size" };

/** The Settings modal: a scope (an agent's name, or `_all`) and, when the URL names one, a section. */
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

/** The Inbox on its first folder, showing every agent's threads with nothing open. */
export const INBOX: InboxPlace = {
  kind: "inbox",
  folder: "inbox",
  agent: null,
  thread: null,
  compose: null,
};

/** The ref of message `id` in `agent`'s user inbox, which is what an old `item=` link names. */
export function userInboxRef(agent: string, id: string): string {
  return `${agent}/user/${id}`;
}

/** A location with no panel and no modal, for going to `place`. */
export function locationAt(place: Place): AppLocation {
  return { place, panel: null, settings: null };
}

export function isAgentPlace(place: Place): place is AgentPlace {
  return (
    place.kind === "chat" ||
    place.kind === "activity" ||
    place.kind === "schedule" ||
    place.kind === "files"
  );
}

/** The agent a place belongs to, or null on the places with no viewed agent. */
export function viewedAgentOf(place: Place): string | null {
  return isAgentPlace(place) ? place.agent : null;
}

/** Mirrors the gateway's artifact-name rule, so a bad URL corrects to the list. */
const ARTIFACT_NAME = /^[a-z0-9]+(-[a-z0-9]+)*$/;

export function isArtifactName(value: string): boolean {
  return value.length <= 64 && ARTIFACT_NAME.test(value);
}

/** Whether `name` is a well-formed agent folder name (whether or not such an agent exists). */
export function isAgentName(name: string): boolean {
  return isAgentSlug(name);
}

/**
 * Whether a panel can show on a place: a session of the viewed agent (or on
 * the Workbench), a new session on the viewed agent, a file, or the
 * conversation size. A file that names its agent also shows on the Inbox, for
 * a mail attachment beside its thread.
 */
export function panelAllowed(place: Place, panel: Panel): boolean {
  switch (panel.kind) {
    case "session":
      return (isAgentPlace(place) && place.agent === panel.agent) || place.kind === "workbench";
    case "new-session":
      return isAgentPlace(place) && place.agent === panel.agent;
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

// ── Reading ──────────────────────────────────────────────────────────

/** What a URL is read against. */
export interface ParseContext {
  /**
   * The last-used agent once the agent list is known (the remembered agent when
   * it still exists, else the first by name). Null until then, and when there
   * are no agents.
   */
  lastUsed: string | null;
}

export interface ParsedUrl {
  location: AppLocation;
  /** Plain-language corrections to tell the user about, as toasts. */
  notices: string[];
  /**
   * The URL resolves under the last-used agent, which isn't known yet.
   * `location` is a placeholder until it is.
   */
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
  if (kind === "new-session")
    return isAgentName(rest) ? { kind: "new-session", agent: rest } : null;
  if (kind === "session") {
    const split = rest.indexOf(":");
    if (split < 0) return null;
    const agent = rest.slice(0, split);
    const runId = rest.slice(split + 1);
    return isAgentName(agent) && runId !== "" ? { kind: "session", agent, runId } : null;
  }
  return null;
}

/** The Settings target a `settings` value names. A scope that can't be an agent becomes the install-wide one. */
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

/** The kinds of inbox a message ref names: `user` holds what agents sent the user, `agent` what was sent to an agent. */
const REF_KINDS: ReadonlySet<string> = new Set(["user", "agent"]);

/** A message ref, `<agent>/<kind>/<id>`, or null when `value` isn't shaped like one. */
function parseThreadRef(value: string | null): string | null {
  if (value === null) return null;
  const parts = value.split("/");
  if (parts.length !== 3) return null;
  const [agent = "", kind = "", id = ""] = parts;
  return isAgentName(agent) && REF_KINDS.has(kind) && id !== "" ? value : null;
}

/** An old `item=<agent>:<id>` value as the ref of that message, or null when it is malformed. */
function refOfOldItem(value: string | null): string | null {
  const colon = value === null ? -1 : value.indexOf(":");
  if (value === null || colon <= 0) return null;
  const agent = value.slice(0, colon);
  const id = value.slice(colon + 1);
  return isAgentName(agent) && id !== "" ? userInboxRef(agent, id) : null;
}

/** The agents a `to` value lists, comma separated: the well-formed names once each, in order. */
function parseRecipients(value: string | null): string[] {
  if (value === null) return [];
  const names = value.split(",").filter(isAgentName);
  return names.filter((name, index) => names.indexOf(name) === index);
}

/**
 * The Inbox a query stands for. `folder`, `agent`, `thread` or `compose` with
 * `to` are the current form. The old form, `tab=archived` and
 * `item=<agent>:<id>`, is read as the folder and thread it stands for, and the
 * current parameter wins when both are given.
 */
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

/** What a path stands for, before the query's panel and settings parameters are applied. */
interface PathResult {
  place: Place;
  /** A panel an old path implies, which replaces the query's. */
  panel?: Panel;
  /** A Settings target an old path implies, which replaces the query's. */
  settings?: SettingsTarget;
  /** Set on an old path that resolves under the last-used agent: what it stands for once that agent is known. */
  underLastUsed?: (agent: string) => PathResult;
}

function workbenchPlace(artifact: string | undefined): Place {
  return artifact !== undefined && isArtifactName(artifact)
    ? { kind: "workbench", artifact }
    : { kind: "workbench", artifact: null };
}

/**
 * The Settings target an old settings URL stands for. `agent` is the agent an
 * agent-scope target belongs to. `needsAgent` is set when the target is an
 * agent's and none was given.
 */
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
    // A section only an agent has: it opens on the last-used agent.
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

/** Paths from before the agent and team prefixes: the same places, under the last-used agent. */
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
    // The macOS notification's "Open" link: the notified result is in an agent inbox, reached through Files.
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

/**
 * Read a URL into a location. Old paths are read as the place they redirect to;
 * the ones that resolve under the last-used agent wait (`needsLastUsed`) until
 * it is known. A panel that can't show on its place is dropped, and a path the
 * app doesn't have, or can't read, is the Not Found place at that path.
 */
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

// ── Corrections that depend on what exists ───────────────────────────

/**
 * Move a location off agents that don't exist: an agent place goes to Home, an
 * inbox filter or a New message's recipient is dropped, a Settings scope
 * becomes All agents, and a session panel or a file panel for a missing agent
 * is removed. Each missing agent is named once in the notices.
 */
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
    let { agent, compose } = place;
    if (agent !== null && !known.has(agent)) {
      note(agent);
      agent = null;
    }
    if (compose !== null) {
      const to = compose.to.filter((name) => {
        note(name);
        return known.has(name);
      });
      if (to.length !== compose.to.length) compose = { to };
    }
    place = { ...place, agent, compose };
  }
  const panelAgent = panel?.kind === "session" || panel?.kind === "file" ? panel.agent : null;
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

/** Move the Workbench off an artifact that isn't in the loaded list, back to the list. */
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

// ── Formatting ───────────────────────────────────────────────────────

/** A query value, with the separators this format uses left readable. */
function queryValue(value: string): string {
  return encodeURIComponent(value).replace(/%3A/gi, ":").replace(/%2F/gi, "/");
}

function formatPanel(panel: Panel): string {
  switch (panel.kind) {
    case "size":
      return "size";
    case "file":
      return panel.agent === null
        ? `file:${panel.path}`
        : `agent-file:${panel.agent}:${panel.path}`;
    case "session":
      return `session:${panel.agent}:${panel.runId}`;
    case "new-session":
      return `new-session:${panel.agent}`;
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
        // Encoded in full, separators included: a ref is one value, not a path.
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

/** The URL (path and query) for a location. */
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

/** Whether two places are the same destination, whatever panel or modal is over them. */
export function placesEqual(a: Place, b: Place): boolean {
  return formatLocation(locationAt(a)) === formatLocation(locationAt(b));
}
