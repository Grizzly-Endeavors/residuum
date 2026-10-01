import type {
  ActionInfo,
  PulseInfo,
  ScheduledCurrentRun,
  ScheduledRunOutcome,
  SessionSummary,
} from "../src/lib/generated/protocol";
import type { MockClock } from "./env";
import { json, readJsonObject } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockState } from "./state";
import { instantOfLocal, localMs } from "./zone";

/** A pulse as HEARTBEAT.yml declares it, and when it last ran (`pulse_state.json`). */
export interface MockPulse {
  name: string;
  enabled: boolean;
  /** `null` for a pulse that failed to load. */
  schedule: string | null;
  activeHours: string | null;
  agent: string | null;
  lastRunAt: string | null;
  problems: string[];
}

/** An action as the action store holds it: the listing without its live run. */
export type MockAction = Omit<ActionInfo, "current_run">;

/**
 * The pulses and scheduled actions of an agent that has run. Runs come from
 * the agent's sessions, the way the backend reads them from its session store.
 */
export interface MockScheduled {
  pulses: MockPulse[];
  actions: MockAction[];
}

const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;
const DAY_MS = 24 * HOUR_MS;

export function createScheduled(clock: MockClock): MockScheduled {
  return {
    pulses: [
      {
        name: "inbox_check",
        enabled: true,
        schedule: "2h",
        activeHours: "08:00-22:00",
        agent: null,
        lastRunAt: clock.isoAgo(HOUR_MS),
        problems: [],
      },
      {
        name: "nightly_review",
        enabled: false,
        schedule: "24h",
        activeHours: null,
        agent: "researcher",
        lastRunAt: clock.isoAgo(20 * HOUR_MS),
        problems: [],
      },
      {
        name: "legacy_identity",
        enabled: false,
        schedule: null,
        activeHours: null,
        agent: null,
        lastRunAt: null,
        problems: ["pulse 'legacy_identity' sets include_identity, which has been removed"],
      },
    ],
    actions: [
      {
        id: "act-7f3a2c",
        name: "weekly_digest",
        run_at: clock.isoIn(3 * HOUR_MS),
        agent: null,
        model_tier: null,
      },
      {
        id: "act-91be04",
        name: "review_open_prs",
        run_at: clock.isoIn(26 * HOUR_MS),
        agent: "researcher",
        model_tier: "fast",
      },
    ],
  };
}

/** The `parse_schedule_duration` of the backend, in milliseconds, or `null` for a schedule it refuses. */
function scheduleMs(schedule: string): number | null {
  const match = /^(\d+)([smhd])$/.exec(schedule);
  const value = Number(match?.[1]);
  if (match === null || value <= 0) return null;
  return (
    value * { s: 1000, m: 60_000, h: HOUR_MS, d: 24 * HOUR_MS }[match[2] as "s" | "m" | "h" | "d"]
  );
}

/** The run of `source` (`pulse:<name>`, `action:<name>`) that is live now. */
function currentRun(state: MockState, source: string): ScheduledCurrentRun | null {
  const run = state.sessions.live.find(
    (s) => s.category === "scheduled" && s.source_label === source,
  );
  return run === undefined
    ? null
    : { address: run.address, run_id: run.run_id, overlap: run.overlap };
}

/** The newest finished run of `source`, as the Scheduled view shows its outcome. */
function lastOutcome(state: MockState, source: string): ScheduledRunOutcome | null {
  const run: SessionSummary | undefined = state.sessions.completed.find(
    (s) => s.category === "scheduled" && s.source_label === source,
  );
  if (run === undefined) return null;
  return {
    status: run.outcome ?? "completed",
    at: run.completed_at ?? run.started_at,
    error: run.error,
  };
}

/** The `parse_active_hours` of the backend: the window as minutes into the day, or `null` for one it refuses. */
function parseActiveHours(hours: string): readonly [number, number] | null {
  const match = /^(\d+):(\d+)-(\d+):(\d+)$/.exec(hours);
  if (match === null) return null;
  const [startHour, startMinute, endHour, endMinute] = match.slice(1).map(Number) as [
    number,
    number,
    number,
    number,
  ];
  if (startHour > 23 || endHour > 23 || startMinute > 59 || endMinute > 59) return null;
  return [startHour * 60 + startMinute, endHour * 60 + endMinute];
}

/**
 * The first wall-clock moment at or after `local` inside the window, `null`
 * for a window that never opens: `next_active_moment` in `src/pulse/next_run.rs`.
 */
function intoActiveHours(local: number, [start, end]: readonly [number, number]): number | null {
  if (start === end) return null;
  const day = Math.floor(local / DAY_MS) * DAY_MS;
  const time = (local - day) / MINUTE_MS;
  const inside = start < end ? time >= start && time < end : time >= start || time < end;
  if (inside) return local;
  const opensToday = day + start * MINUTE_MS;
  return local < opensToday ? opensToday : opensToday + DAY_MS;
}

/**
 * When the pulse next runs, as an instant in milliseconds, or `null` when it
 * will not: `next_run_at` in `src/pulse/next_run.rs`, the one calculation behind
 * the Scheduled view's `next_fire_at` and the overview's `upcoming`.
 *
 * It is the first moment at or after the later of `nowMs` and the end of the
 * schedule since the last run that falls inside the active hours, read in the
 * hub's timezone. A pulse that is due already is reported at the start of the
 * current minute.
 */
export function nextPulseRun(pulse: MockPulse, nowMs: number): number | null {
  const interval = pulse.schedule === null ? null : scheduleMs(pulse.schedule);
  if (!pulse.enabled || interval === null) return null;
  const window = pulse.activeHours === null ? null : parseActiveHours(pulse.activeHours);
  if (pulse.activeHours !== null && window === null) return null;

  const thisMinute = Math.floor(nowMs / MINUTE_MS) * MINUTE_MS;
  const nowLocal = localMs(thisMinute);
  const due =
    pulse.lastRunAt === null
      ? nowLocal
      : Math.max(localMs(Date.parse(pulse.lastRunAt)) + interval, nowLocal);
  const at = window === null ? due : intoActiveHours(due, window);
  if (at === null) return null;
  return at <= nowLocal ? thisMinute : instantOfLocal(at);
}

function nextFireAt(pulse: MockPulse, clock: MockClock): string | null {
  const at = nextPulseRun(pulse, clock.now());
  return at === null ? null : new Date(at).toISOString();
}

function pulseInfo(state: MockState, pulse: MockPulse): PulseInfo {
  const source = `pulse:${pulse.name}`;
  return {
    name: pulse.name,
    enabled: pulse.enabled,
    schedule: pulse.schedule,
    active_hours: pulse.activeHours,
    agent: pulse.agent,
    next_fire_at: nextFireAt(pulse, state.env.clock),
    last_outcome: lastOutcome(state, source),
    current_run: currentRun(state, source),
    problems: pulse.problems,
  };
}

/**
 * A running agent's file watcher sees an edit to its pulses or its actions,
 * and the hub reads its schedule again. Nothing watches a stopped agent's, so
 * its schedule is read the next time the overview is asked for.
 */
function noticeScheduleChange(ctx: RouteContext): void {
  const agent = ctx.hub.agents.get(ctx.state.agentName);
  if (agent?.runState === "running") ctx.hub.overview.changed(agent);
}

/** `PUT .../scheduled/pulses/{pulse}/enabled`: body `{ enabled }`; answers `{ name, enabled }`. */
async function setPulseEnabled(ctx: RouteContext): Promise<void> {
  const { res, state } = ctx;
  let enabled: unknown;
  try {
    enabled = (await readJsonObject(ctx.req)).enabled;
    if (typeof enabled !== "boolean") throw new Error("missing boolean field `enabled`");
  } catch (err) {
    const why = err instanceof Error ? err.message : String(err);
    json(res, 400, { error: `invalid request body: ${why}` });
    return;
  }
  const name = decodedParam(ctx, 0);
  const pulse = state.scheduled.pulses.find((p) => p.name === name);
  if (pulse === undefined) {
    json(res, 404, { error: `No pulse named "${name}" was found in HEARTBEAT.yml.` });
    return;
  }
  pulse.enabled = enabled;
  noticeScheduleChange(ctx);
  json(res, 200, { name, enabled });
}

/** `DELETE .../scheduled/actions/{id}`: cancel a pending action. */
function cancelAction(ctx: RouteContext): void {
  const { res, state } = ctx;
  const id = decodedParam(ctx, 0);
  const at = state.scheduled.actions.findIndex((a) => a.id === id);
  if (at === -1) {
    json(res, 404, { error: `No scheduled action with id "${id}" was found.` });
    return;
  }
  state.scheduled.actions.splice(at, 1);
  noticeScheduleChange(ctx);
  json(res, 200, { id, cancelled: true });
}

/** The Scheduled view routes, in the unscoped `/api/...` spelling. */
export const scheduledRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/scheduled/pulses",
    handler: ({ res, state }) => {
      json(
        res,
        200,
        state.scheduled.pulses.map((pulse) => pulseInfo(state, pulse)),
      );
    },
  },
  {
    method: "PUT",
    pattern: /^\/api\/scheduled\/pulses\/([^/]+)\/enabled$/,
    handler: setPulseEnabled,
  },
  {
    method: "GET",
    pattern: "/api/scheduled/actions",
    handler: ({ res, state }) => {
      json(
        res,
        200,
        state.scheduled.actions.map(
          (action): ActionInfo => ({
            ...action,
            current_run: currentRun(state, `action:${action.name}`),
          }),
        ),
      );
    },
  },
  { method: "DELETE", pattern: /^\/api\/scheduled\/actions\/([^/]+)$/, handler: cancelAction },
];
