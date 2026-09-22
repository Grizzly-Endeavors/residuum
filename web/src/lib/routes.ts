// URL <-> app location. The chat side and the settings page are tracked
// separately so that leaving settings returns to the chat side exactly as it
// was (same session, workspace open or not), matching how the layout keeps
// the chat, session view, and workspace mounted underneath.
//
// Paths:
//   /                         main chat
//   /sessions/:runId          a session's run in the main pane
//   ?workspace                workspace panel open beside either of the above
//   /settings/:section        settings (bare /settings opens the first section)

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
};

const DEFAULT_SECTION: SettingsSection = "runtime";

/** What the chat side of the app shows. */
export interface ChatLocation {
  /** The run shown in the main pane, or null for the main chat. */
  runId: string | null;
  workspace: boolean;
}

export interface AppLocation {
  chat: ChatLocation;
  /** The settings section shown, or null when not on the settings page. */
  settings: SettingsSection | null;
}

export interface ParsedLocation {
  location: AppLocation;
  /**
   * The URL didn't name a known place (an unknown path or settings section),
   * so the address bar should be corrected to the formatted location.
   */
  corrected: boolean;
}

export const MAIN_CHAT: ChatLocation = { runId: null, workspace: false };

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

/**
 * Read a URL into a location. A settings URL says nothing about the chat
 * side, so `currentChat` carries over unchanged.
 */
export function parseLocation(
  pathname: string,
  search: string,
  currentChat: ChatLocation,
): ParsedLocation {
  const segments = pathname.split("/").filter((s) => s !== "");
  const workspace = new URLSearchParams(search).has("workspace");
  const [first, second, ...rest] = segments;

  if (first === undefined) {
    return { location: { chat: { runId: null, workspace }, settings: null }, corrected: false };
  }

  if (first === "settings" && rest.length === 0) {
    const section = second === undefined ? null : decodeSegment(second);
    if (section !== null && isSettingsSection(section)) {
      return { location: { chat: currentChat, settings: section }, corrected: false };
    }
    return { location: { chat: currentChat, settings: DEFAULT_SECTION }, corrected: true };
  }

  if (first === "sessions" && second !== undefined && rest.length === 0) {
    const runId = decodeSegment(second);
    if (runId !== null && runId !== "") {
      return { location: { chat: { runId, workspace }, settings: null }, corrected: false };
    }
  }

  return { location: { chat: { runId: null, workspace }, settings: null }, corrected: true };
}

/** The URL (path and query) for a location. */
export function formatLocation(location: AppLocation): string {
  if (location.settings !== null) return `/settings/${location.settings}`;
  const { runId, workspace } = location.chat;
  const path = runId === null ? "/" : `/sessions/${encodeURIComponent(runId)}`;
  return workspace ? `${path}?workspace` : path;
}
