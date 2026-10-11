// The browser tab's title: where the user is, and, while the tab is hidden,
// what they would want to come back for. The words are worked out here; the
// signals they come from are read in `shell/TabTitle.svelte`.

import type { Place } from "./routes";

export const APP_NAME = "Residuum";

export interface TabTitleInput {
  /** The agent or place the page is on, or null on Home. */
  subject: string | null;
  /** The tab is in the background. */
  hidden: boolean;
  /** What is unread across the agents' chats and the inbox. */
  unread: number;
  /** The viewed agent is in a turn. */
  working: boolean;
  /** The viewed agent ended a turn while the tab was hidden. */
  finished: boolean;
}

/**
 * What the tab is titled with: the subject, then the app (`atlas · Residuum`).
 * While the tab is hidden an unread count leads (`(2) atlas · Residuum`), and
 * the agent being busy or having just finished follows its name. Visible, the
 * title is the same words with no markers: the page itself says the rest.
 */
export function tabTitle({ subject, hidden, unread, working, finished }: TabTitleInput): string {
  const count = hidden && unread > 0 ? `(${String(unread)}) ` : "";
  if (subject === null) return `${count}${APP_NAME}`;
  return `${count}${subject}${hiddenStatus({ hidden, working, finished })} · ${APP_NAME}`;
}

/** What follows the agent's name in a hidden tab: busy, just done, or nothing. */
function hiddenStatus({
  hidden,
  working,
  finished,
}: Pick<TabTitleInput, "hidden" | "working" | "finished">): string {
  if (!hidden) return "";
  if (working) return " is working";
  return finished ? " finished" : "";
}

/** What a place is called in the title: the agent for its places, the place's own name elsewhere, nothing on Home. */
export function placeSubject(place: Place, agentName: (agent: string) => string): string | null {
  switch (place.kind) {
    case "home":
      return null;
    case "inbox":
      return "Inbox";
    case "workbench":
      return "Workbench";
    case "shared-files":
      return "Shared files";
    case "chat":
    case "activity":
    case "schedule":
    case "files":
      return agentName(place.agent);
  }
}
