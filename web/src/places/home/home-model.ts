import type {
  AgentActivity,
  AgentErrorKind,
  AgentOverview,
  AgentSummary,
  LastMessage,
  TeamEventTarget,
  UpcomingRun,
} from "../../lib/hub-types";
import { displayState } from "../../lib/agent-display-state";
import { INBOX, userInboxRef, type AppLocation } from "../../lib/routes";
import type { StatusDotState } from "../../lib/ui";

const MINUTE_MS = 60_000;
const DAY_MS = 24 * 60 * MINUTE_MS;

const clockFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const yearDayFormat = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  year: "numeric",
});
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short" });

function dayStart(ms: number): number {
  const date = new Date(ms);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

function daysFrom(now: number, ms: number): number {
  return Math.round((dayStart(ms) - dayStart(now)) / DAY_MS);
}

function shortDate(ms: number, now: number): string {
  const sameYear = new Date(ms).getFullYear() === new Date(now).getFullYear();
  return (sameYear ? dayFormat : yearDayFormat).format(ms);
}

export function pastWhen(at: string, now: number): string {
  const ms = Date.parse(at);
  if (Number.isNaN(ms)) return "";
  return daysFrom(now, ms) === 0 ? clockFormat.format(ms) : shortDate(ms, now);
}

export function lastMessageWhen(message: LastMessage, now: number): string {
  if (message.at_precision === "minute") return pastWhen(message.at, now);
  const day = message.at.slice(0, 10);
  const [year, month, date] = day.split("-").map(Number);
  if (year === undefined || month === undefined || date === undefined) return "";
  const ms = new Date(year, month - 1, date).getTime();
  return daysFrom(now, ms) === 0 ? "Today" : shortDate(ms, now);
}

export function upcomingWhen(at: string, now: number): string {
  const ms = Date.parse(at);
  if (Number.isNaN(ms)) return "";
  if (ms <= now) return "Due now";
  if (ms - now < 60 * MINUTE_MS)
    return `in ${String(Math.max(1, Math.round((ms - now) / MINUTE_MS)))}m`;
  const clock = clockFormat.format(ms);
  const days = daysFrom(now, ms);
  if (days === 0) return `Today at ${clock}`;
  if (days === 1) return `Tomorrow at ${clock}`;
  if (days < 7) return `${weekdayFormat.format(ms)} at ${clock}`;
  return `${shortDate(ms, now)} at ${clock}`;
}

export function elapsed(since: string, now: number): string {
  const seconds = Math.max(0, Math.floor((now - Date.parse(since)) / 1000));
  if (Number.isNaN(seconds)) return "";
  if (seconds < 60) return `${String(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${String(minutes)}m`;
  const rest = minutes % 60;
  return `${String(Math.floor(minutes / 60))}h${rest > 0 ? ` ${String(rest)}m` : ""}`;
}

export interface TeamTally {
  running: number;
  stopped: number;
  failed: number;
}

export function teamTally(agents: readonly AgentSummary[]): TeamTally {
  const tally: TeamTally = { running: 0, stopped: 0, failed: 0 };
  for (const agent of agents) {
    if (agent.state === "stopped") tally.stopped += 1;
    else if (agent.state === "failed") tally.failed += 1;
    else tally.running += 1;
  }
  return tally;
}

export const STATE_WORDS: Readonly<Record<StatusDotState, string>> = {
  running: "Running",
  starting: "Starting",
  stopping: "Stopping",
  stopped: "Stopped",
  failed: "Can't start",
};

const FAILURE_SHORT: Readonly<Record<AgentErrorKind, string>> = {
  config: "Its settings need fixing",
  port_conflict: "A port it needs is taken",
  crash: "It stopped unexpectedly",
  other: "Something stopped it starting",
};

export type NowTone = "default" | "quiet" | "accent" | "danger";

export interface NowLine {
  text: string;
  tone: NowTone;
  echo: boolean;
}

export interface RowInput {
  agent: AgentSummary;
  activity: AgentActivity;
  stopping: boolean;
  overview: AgentOverview | undefined;
  now: number;
}

export function nowLine(input: RowInput): NowLine {
  const { agent, activity, overview, now } = input;
  switch (displayState(agent.state, input.stopping)) {
    case "failed":
      return {
        text: FAILURE_SHORT[agent.last_error?.kind ?? "other"],
        tone: "danger",
        echo: false,
      };
    case "starting":
      return { text: "Starting…", tone: "accent", echo: true };
    case "stopping":
      return { text: "Stopping…", tone: "quiet", echo: true };
    case "stopped":
      return { text: "Not running", tone: "quiet", echo: true };
    case "running":
      break;
  }
  if (activity.busy) {
    const since = activity.busy_since === null ? "" : ` · ${elapsed(activity.busy_since, now)}`;
    if (activity.restart_pending === true) {
      return { text: `Restarting after this turn${since}`, tone: "accent", echo: false };
    }
    return { text: `Working on a reply${since}`, tone: "accent", echo: false };
  }
  const newest = overview?.live_sessions.at(-1);
  if (newest !== undefined) return { text: newest.purpose, tone: "default", echo: false };
  return { text: "Idle", tone: "quiet", echo: false };
}

export function lastLine(
  overview: AgentOverview | undefined,
  now: number,
): { when: string; text: string } | null {
  const message = overview?.last_message;
  if (!message) return null;
  return {
    when: lastMessageWhen(message, now),
    text: message.role === "user" ? `You: ${message.preview}` : message.preview,
  };
}

export function runTitle(name: string): string {
  const words = name.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function runWhen(run: UpcomingRun, state: StatusDotState, now: number): string {
  if (state === "running" || state === "starting") return upcomingWhen(run.at, now);
  if (Date.parse(run.at) <= now) return "Runs when it starts";
  return state === "failed" ? "Won't run until it starts" : "Won't run while stopped";
}

export const COMING_UP_LIMIT = 8;

export interface ComingRun {
  agent: string;
  run: UpcomingRun;
}

export function comingUp(
  agents: readonly AgentSummary[],
  overviews: Readonly<Record<string, AgentOverview>>,
): ComingRun[] {
  const runs = agents
    .filter((agent) => agent.state === "running" || agent.state === "starting")
    .flatMap((agent) =>
      (overviews[agent.name]?.upcoming ?? []).map((run) => ({ agent: agent.name, run })),
    );
  runs.sort((a, b) => Date.parse(a.run.at) - Date.parse(b.run.at));
  return runs.slice(0, COMING_UP_LIMIT);
}

export function eventLocation(target: TeamEventTarget): AppLocation {
  switch (target.kind) {
    case "agent_place":
      return {
        place: { kind: target.place, agent: target.agent },
        panel: null,
        settings: null,
      };
    case "session":
      return {
        place: { kind: "chat", agent: target.agent },
        panel: { kind: "session", agent: target.agent, runId: target.run_id },
        settings: null,
      };
    case "inbox_item":
      return {
        place: { ...INBOX, thread: userInboxRef(target.agent, target.item_id) },
        panel: null,
        settings: null,
      };
  }
}

export function splitSummary(
  summary: string,
  agent: string | null,
): { agent: string | null; rest: string } {
  if (agent !== null && summary.startsWith(agent)) {
    return { agent, rest: summary.slice(agent.length) };
  }
  return { agent: null, rest: summary };
}
