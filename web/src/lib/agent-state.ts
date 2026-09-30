// How an agent's state reads in words, for labels that don't lean on color.

import type { AgentState, AgentSummary } from "./hub-types";

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

/**
 * The full spoken description of an agent: "scout, running, working, 3 unread".
 * A failed agent's last error is included so it reads the same on focus as on
 * hover.
 */
export function describeAgent(
  agent: AgentSummary,
  activity: { busy: boolean; unread: number },
): string {
  const parts = [agent.name, stateLabel(agent.state)];
  if (activity.busy) parts.push("working");
  if (activity.unread > 0) parts.push(`${activity.unread} unread`);
  if (agent.state === "failed" && agent.last_error) {
    parts.push(`last error: ${agent.last_error.message}`);
  }
  return parts.join(", ");
}
