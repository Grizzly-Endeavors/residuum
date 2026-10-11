// What Home says about the team, worked out from the hub's agent list and the
// overview: the header's tally, each board row's words, the runs coming up,
// where a team event leads, and how Home writes times.

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
import type { AppLocation } from "../../lib/routes";
import type { StatusDotState } from "../../lib/ui";

// ── Times ────────────────────────────────────────────────────────────

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

/** Local midnight of the day `ms` falls on. */
function dayStart(ms: number): number {
  const date = new Date(ms);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** Whole local days from `now`'s day to `ms`'s day: 0 today, 1 tomorrow, -1 yesterday. */
function daysFrom(now: number, ms: number): number {
  return Math.round((dayStart(ms) - dayStart(now)) / DAY_MS);
}

/** A date without the year when it is this year's: "Sep 24", or "Sep 24, 2025". */
function shortDate(ms: number, now: number): string {
  const sameYear = new Date(ms).getFullYear() === new Date(now).getFullYear();
  return (sameYear ? dayFormat : yearDayFormat).format(ms);
}

/** When something happened: the clock time today, the date before. */
export function pastWhen(at: string, now: number): string {
  const ms = Date.parse(at);
  if (Number.isNaN(ms)) return "";
  return daysFrom(now, ms) === 0 ? clockFormat.format(ms) : shortDate(ms, now);
}

/** When the last message was written. A message known only to the day reads as that day. */
export function lastMessageWhen(message: LastMessage, now: number): string {
  if (message.at_precision === "minute") return pastWhen(message.at, now);
  // A day-precise time is the start of that day in the hub's zone; its date is what counts.
  const day = message.at.slice(0, 10);
  const [year, month, date] = day.split("-").map(Number);
  if (year === undefined || month === undefined || date === undefined) return "";
  const ms = new Date(year, month - 1, date).getTime();
  return daysFrom(now, ms) === 0 ? "Today" : shortDate(ms, now);
}

/** When a run comes up: "Due now", "in 34m", "Today at 9:00 AM", "Tomorrow at 8:00 AM", "Fri at 9:00 AM", "Oct 2 at 9:00 AM". */
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

/** How long something has been going: "14s", "3m", "2h 5m". */
export function elapsed(since: string, now: number): string {
  const seconds = Math.max(0, Math.floor((now - Date.parse(since)) / 1000));
  if (Number.isNaN(seconds)) return "";
  if (seconds < 60) return `${String(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${String(minutes)}m`;
  const rest = minutes % 60;
  return `${String(Math.floor(minutes / 60))}h${rest > 0 ? ` ${String(rest)}m` : ""}`;
}

// ── The header ───────────────────────────────────────────────────────

export interface TeamTally {
  running: number;
  stopped: number;
  failed: number;
}

/** How many agents run, are stopped, and can't start. A starting agent counts as running. */
export function teamTally(agents: readonly AgentSummary[]): TeamTally {
  const tally: TeamTally = { running: 0, stopped: 0, failed: 0 };
  for (const agent of agents) {
    if (agent.state === "stopped") tally.stopped += 1;
    else if (agent.state === "failed") tally.failed += 1;
    else tally.running += 1;
  }
  return tally;
}

// ── A board row ──────────────────────────────────────────────────────

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
  /** It only repeats the state word, so a line that already shows the state leaves it out. */
  echo: boolean;
}

export interface RowInput {
  agent: AgentSummary;
  activity: AgentActivity;
  stopping: boolean;
  overview: AgentOverview | undefined;
  now: number;
}

/** What the agent is doing now. */
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
    return { text: `Working on a reply${since}`, tone: "accent", echo: false };
  }
  const newest = overview?.live_sessions.at(-1);
  if (newest !== undefined) return { text: newest.purpose, tone: "default", echo: false };
  return { text: "Idle", tone: "quiet", echo: false };
}

/** The last message under the Now line: when, and its preview with the user's own marked. */
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

/** A pulse's or action's name as words: `inbox_check` reads "Inbox check". */
export function runTitle(name: string): string {
  const words = name.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** When a run comes up for an agent in `state`: a run of an agent that isn't running waits for it. */
export function runWhen(run: UpcomingRun, state: StatusDotState, now: number): string {
  if (state === "running" || state === "starting") return upcomingWhen(run.at, now);
  if (Date.parse(run.at) <= now) return "Runs when it starts";
  return state === "failed" ? "Won't run until it starts" : "Won't run while stopped";
}

// ── Coming up ────────────────────────────────────────────────────────

/** The most runs Coming up lists. */
export const COMING_UP_LIMIT = 8;

export interface ComingRun {
  agent: string;
  run: UpcomingRun;
}

/** The soonest runs across the agents that will run them, soonest first. */
export function comingUp(
  agents: readonly AgentSummary[],
  overviews: Readonly<Record<string, AgentOverview>>,
): ComingRun[] {
  const runs = agents
    .filter((agent) => agent.state === "running" || agent.state === "starting")
    .flatMap((agent) =>
      (overviews[agent.name]?.upcoming ?? []).map((run) => ({ agent: agent.name, run })),
    );
  // The agents come sorted by name and the sort is stable, so runs at the same moment keep that order.
  runs.sort((a, b) => Date.parse(a.run.at) - Date.parse(b.run.at));
  return runs.slice(0, COMING_UP_LIMIT);
}

// ── Team events ──────────────────────────────────────────────────────

/** Where a team event's target is in the app. */
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
        place: {
          kind: "inbox",
          agent: null,
          tab: "active",
          item: { agent: target.agent, id: target.item_id },
        },
        panel: null,
        settings: null,
      };
  }
}

/**
 * A summary split around its leading agent name, so the name can stand out:
 * "atlas started" is `{ agent: "atlas", rest: " started" }`.
 */
export function splitSummary(
  summary: string,
  agent: string | null,
): { agent: string | null; rest: string } {
  if (agent !== null && summary.startsWith(agent)) {
    return { agent, rest: summary.slice(agent.length) };
  }
  return { agent: null, rest: summary };
}
