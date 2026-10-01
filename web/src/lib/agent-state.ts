// How an agent's state reads in words, for labels that don't lean on color.

import type { AgentState } from "./hub-types";

const STATE_LABELS: Record<AgentState, string> = {
  running: "running",
  starting: "starting",
  stopped: "stopped",
  failed: "failed",
};

export function stateLabel(state: AgentState): string {
  return STATE_LABELS[state];
}

/** The largest unread count shown as a number; above it the badge reads "99+". */
const MAX_UNREAD_SHOWN = 99;

export function unreadText(unread: number): string {
  return unread > MAX_UNREAD_SHOWN ? `${MAX_UNREAD_SHOWN}+` : String(unread);
}
