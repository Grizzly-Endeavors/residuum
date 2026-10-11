// What the Schedule place says about a pulse or a scheduled action: how often
// it runs, when it runs next, and how its last run went. Names and next-run
// times read the way Home's do.

import type { IconName } from "../../lib/icons";
import { relativeTime } from "../../lib/time";
import type {
  ActionInfo,
  PulseInfo,
  ScheduledCurrentRun,
  ScheduledRunOutcome,
} from "../../lib/types";
import type { StatusDotState } from "../../lib/ui";
import { runWhen } from "../home/home-model";

const UNITS = { s: "second", m: "minute", h: "hour", d: "day" } as const;

/** How often a pulse runs: "Every 2 hours", "Every day, between 02:00 and 06:00". `null` without a schedule. */
export function pulseCadence(pulse: PulseInfo): string | null {
  if (pulse.schedule === null) return null;
  const match = /^(\d+)([smhd])$/.exec(pulse.schedule);
  const unit = match === null ? null : UNITS[match[2] as keyof typeof UNITS];
  const count = Number(match?.[1]);
  let every = `Every ${pulse.schedule}`;
  if (unit !== null) every = count === 1 ? `Every ${unit}` : `Every ${String(count)} ${unit}s`;
  if (pulse.active_hours === null) return every;
  const [from, to] = pulse.active_hours.split("-");
  return from && to ? `${every}, between ${from} and ${to}` : `${every}, ${pulse.active_hours}`;
}

/**
 * When a pulse runs next, as Home says it ("in 34m", "Due now", "Won't run
 * while stopped"), or why it won't run.
 */
export function pulseNext(pulse: PulseInfo, state: StatusDotState, now: number): string {
  if (pulse.schedule === null) return "Won't run until fixed";
  if (!pulse.enabled) return "Paused";
  if (pulse.next_fire_at === null) return "No next run";
  return runWhen({ kind: "pulse", name: pulse.name, at: pulse.next_fire_at }, state, now);
}

/** What the place says in place of the schedule while the agent isn't running. */
export function notRunningTitle(agent: string, state: "stopped" | "failed" | "starting"): string {
  if (state === "failed") return `${agent} can't start`;
  return state === "starting" ? `${agent} is starting` : `${agent} is stopped`;
}

/** A pulse's mark: a warning for one with problems, a pause for one turned off. */
export function pulseIcon(pulse: PulseInfo): IconName {
  if (pulse.problems.length > 0) return "warning";
  return pulse.enabled ? "clock" : "pause";
}

/** When an action is due, as Home says it. */
export function actionWhen(action: ActionInfo, state: StatusDotState, now: number): string {
  return runWhen({ kind: "action", name: action.name, at: action.run_at }, state, now);
}

/**
 * Whether no pulse that should run has a next run. The agent's pulses are
 * turned off in its settings, or its settings can't be read.
 */
export function pulsesHeld(pulses: readonly PulseInfo[]): boolean {
  const scheduled = pulses.filter((pulse) => pulse.enabled && pulse.schedule !== null);
  return scheduled.length > 0 && scheduled.every((pulse) => pulse.next_fire_at === null);
}

/** How the last run went: "Last ran 1h ago", "Last run failed 2d ago: <error>". */
export function lastRun(outcome: ScheduledRunOutcome, now: number): string {
  const when = relativeTime(outcome.at, now);
  switch (outcome.status) {
    case "completed":
      return `Last ran ${when}`;
    case "cancelled":
      return `Last run was stopped ${when}`;
    case "failed":
      return `Last run failed ${when}${outcome.error ? `: ${outcome.error}` : ""}`;
  }
}

/** A note on a run that started before the one before it had finished, or `null`. */
export function overlapNote(run: ScheduledCurrentRun, now: number): string | null {
  if (run.overlap === null) return null;
  const since = relativeTime(run.overlap.previous_started_at, now);
  return `Started while the run from ${since} was still going`;
}
