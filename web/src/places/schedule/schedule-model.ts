import type { IconName } from "../../lib/icons";
import { isTier } from "../../lib/session-format";
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

const WEEKDAYS: Record<string, string> = {
  mon: "Monday",
  tue: "Tuesday",
  wed: "Wednesday",
  thu: "Thursday",
  fri: "Friday",
  sat: "Saturday",
  sun: "Sunday",
};

function dayNames(days: readonly string[]): string {
  const names = days.map((day) => WEEKDAYS[day.slice(0, 3).toLowerCase()] ?? day);
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

function spanWords(schedule: string): string {
  const match = /^(\d+)([smhd])$/.exec(schedule);
  if (match === null) return schedule;
  const unit = UNITS[match[2] as keyof typeof UNITS];
  return `${match[1]} ${unit}${Number(match[1]) === 1 ? "" : "s"}`;
}

export function pulseCadence(pulse: PulseInfo): string | null {
  if (pulse.days !== null) {
    const start = pulse.active_hours?.split("-")[0];
    const at = start ? start : (pulse.active_hours ?? "midnight");
    const gap = pulse.schedule === null ? "" : `, at least ${spanWords(pulse.schedule)} apart`;
    return `Every ${dayNames(pulse.days)} at ${at}${gap}`;
  }
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

export function pulseNext(pulse: PulseInfo, state: StatusDotState, now: number): string {
  if (pulse.schedule === null && pulse.days === null) return "Won't run until fixed";
  if (!pulse.enabled) return "Paused";
  if (pulse.next_fire_at === null) return "No next run";
  return runWhen({ kind: "pulse", name: pulse.name, at: pulse.next_fire_at }, state, now);
}

export function modelLine(model: string | null): string | null {
  if (model === null) return null;
  return isTier(model) ? `Model: ${model} tier` : `Model: ${model}`;
}

export function notRunningTitle(agent: string, state: "stopped" | "failed" | "starting"): string {
  if (state === "failed") return `${agent} can't start`;
  return state === "starting" ? `${agent} is starting` : `${agent} is stopped`;
}

export function pulseIcon(pulse: PulseInfo): IconName {
  if (pulse.problems.length > 0) return "warning";
  return pulse.enabled ? "clock" : "pause";
}

export function actionWhen(action: ActionInfo, state: StatusDotState, now: number): string {
  return runWhen({ kind: "action", name: action.name, at: action.run_at }, state, now);
}

export function pulsesHeld(pulses: readonly PulseInfo[]): boolean {
  const scheduled = pulses.filter(
    (pulse) => pulse.enabled && (pulse.schedule !== null || pulse.days !== null),
  );
  return scheduled.length > 0 && scheduled.every((pulse) => pulse.next_fire_at === null);
}

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

export function runPhase(run: ScheduledCurrentRun): string {
  if (run.phase === "script") return "Running scripts";
  return run.run_id === null ? "Starting session" : "Running session";
}

export function runStarted(run: ScheduledCurrentRun, now: number): string {
  return `Started ${relativeTime(run.started_at, now)}`;
}

export function overlapNote(run: ScheduledCurrentRun, now: number): string | null {
  if (run.overlap === null) return null;
  const since = relativeTime(run.overlap.previous_started_at, now);
  return `Started while the run from ${since} was still going`;
}
