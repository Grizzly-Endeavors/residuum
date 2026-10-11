// What the rail shows about each agent, worked out from the hub's agent list:
// its row's mark, word and badge, and the places listed under it.

import { displayState } from "../lib/agent-display-state";
import type { AgentActivity, AgentSummary } from "../lib/hub-types";
import type { IconName } from "../lib/icons";
import type { AgentPlaceKind } from "../lib/routes";
import type { StatusDotState } from "../lib/ui";

export interface AgentPlaceEntry {
  kind: AgentPlaceKind;
  label: string;
  icon: IconName;
}

/** An agent's places, in the order the rail lists them under the agent. */
export const AGENT_PLACES: readonly AgentPlaceEntry[] = [
  { kind: "chat", label: "Chat", icon: "chat" },
  { kind: "activity", label: "Activity", icon: "activity" },
  { kind: "schedule", label: "Schedule", icon: "clock" },
  { kind: "files", label: "Files", icon: "folder" },
];

export function agentPlaceLabel(kind: AgentPlaceKind): string {
  return AGENT_PLACES.find((entry) => entry.kind === kind)?.label ?? kind;
}

/** What follows an agent's name in its row: its chat-unread count, a word for its state, or nothing. */
export type AgentRowTail =
  | { kind: "unread"; count: number }
  | { kind: "word"; word: string; tone: "danger" | "accent" | "quiet" }
  | { kind: "none" };

export interface AgentRowStatus {
  dot: StatusDotState;
  /** Busy with a main turn: the dot pulses and the agent's places carry a moving vein. */
  working: boolean;
  tail: AgentRowTail;
  /** The state in words, for assistive technology: "running, working, 3 unread". */
  spoken: string;
}

export interface AgentRowContext {
  activity: AgentActivity;
  /** The hub has begun stopping it. */
  stopping: boolean;
  /** It is the viewed agent. Its working word is left out: the user is already looking. */
  viewed: boolean;
  /** Its chat is the place shown, so new replies are already in view. */
  onItsChat: boolean;
}

const STATE_WORDS: Readonly<Record<StatusDotState, string>> = {
  running: "Running",
  starting: "Starting",
  stopping: "Stopping",
  stopped: "Stopped",
  failed: "Failed",
};

/** A state as one capitalized word: "Running", "Stopping". */
export function stateWord(state: StatusDotState): string {
  return STATE_WORDS[state];
}

/**
 * One agent row. Unread replies win the space after the name, then a state
 * that isn't running, then "Working" for a busy agent the user isn't viewing.
 */
export function agentRowStatus(agent: AgentSummary, context: AgentRowContext): AgentRowStatus {
  const dot = displayState(agent.state, context.stopping);
  const working = dot === "running" && context.activity.busy;
  const unread = context.onItsChat ? 0 : context.activity.unread;

  let tail: AgentRowTail = { kind: "none" };
  if (unread > 0) tail = { kind: "unread", count: unread };
  else if (dot === "failed") tail = { kind: "word", word: STATE_WORDS.failed, tone: "danger" };
  else if (dot !== "running") tail = { kind: "word", word: STATE_WORDS[dot], tone: "quiet" };
  else if (working && !context.viewed) tail = { kind: "word", word: "Working", tone: "accent" };

  const spoken = [STATE_WORDS[dot].toLowerCase()];
  if (working) spoken.push("working");
  if (unread > 0) spoken.push(`${String(unread)} unread`);
  return { dot, working, tail, spoken: spoken.join(", ") };
}
