import { displayState } from "../lib/agent-display-state";
import type { AgentActivity, AgentSummary } from "../lib/hub-types";
import type { IconName } from "../lib/icons";
import { AGENT_PANEL_KINDS, type AgentPanelKind, type AgentPlaceKind } from "../lib/routes";
import type { StatusDotState } from "../lib/ui";

export interface AgentPlaceEntry<Kind extends AgentPlaceKind = AgentPlaceKind> {
  kind: Kind;
  label: string;
  icon: IconName;
}

const PLACE_ENTRIES: Record<AgentPlaceKind, { label: string; icon: IconName }> = {
  chat: { label: "Chat", icon: "chat" },
  activity: { label: "Activity", icon: "activity" },
  schedule: { label: "Schedule", icon: "clock" },
  files: { label: "Files", icon: "folder" },
};

export function agentPlaceEntry<Kind extends AgentPlaceKind>(kind: Kind): AgentPlaceEntry<Kind> {
  return { kind, ...PLACE_ENTRIES[kind] };
}

export const AGENT_PLACES: readonly AgentPlaceEntry[] = (
  ["chat", ...AGENT_PANEL_KINDS] as const
).map((kind) => agentPlaceEntry(kind));

export const AGENT_PANEL_PLACES: readonly AgentPlaceEntry<AgentPanelKind>[] = AGENT_PANEL_KINDS.map(
  (kind) => agentPlaceEntry(kind),
);

export type AgentRowTail =
  | { kind: "unread"; count: number }
  | { kind: "word"; word: string; tone: "danger" | "accent" | "quiet" }
  | { kind: "none" };

export interface AgentRowStatus {
  dot: StatusDotState;
  working: boolean;
  tail: AgentRowTail;
  spoken: string;
}

export interface AgentRowContext {
  activity: AgentActivity;
  stopping: boolean;
  viewed: boolean;
  onItsChat: boolean;
}

const STATE_WORDS: Readonly<Record<StatusDotState, string>> = {
  running: "Running",
  starting: "Starting",
  stopping: "Stopping",
  stopped: "Stopped",
  failed: "Failed",
};

export function stateWord(state: StatusDotState): string {
  return STATE_WORDS[state];
}

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
