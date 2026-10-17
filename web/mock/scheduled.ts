import type {
  ActionInfo,
  PulseInfo,
  PulseOverlap,
  ScheduledCurrentRun,
  ScheduledRunOutcome,
  SessionRunStatus,
  SessionSummary,
} from "../src/lib/generated/protocol";
import type { MockClock } from "./env";
import { json, readJsonObject, stringField } from "./http";
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
  /** The pulse's `model_tier`, when it names one. */
  modelTier?: string;
  lastRunAt: string | null;
  problems: string[];
}

/**
 * An action as the action store holds it: the listing without its live run,
 * and with no `problem` unless one is set.
 */
export type MockAction = Omit<ActionInfo, "current_run" | "problem"> & { problem?: string };

/**
 * A pulse fire in flight, running its task scripts. The mock's fires never
 * ask for a session: each ends with an outcome of its own, or is stopped.
 */
export interface MockPulseFire {
  fireId: string;
  pulse: string;
  startedAt: string;
  /** Asked to stop, and finishing. */
  stopping: boolean;
}

/**
 * The pulses and scheduled actions of an agent that has run. Session runs
 * come from the agent's sessions, the way the backend reads them from its
 * session store; fires that run no session are held here.
 */
export interface MockScheduled {
  pulses: MockPulse[];
  actions: MockAction[];
  /** The fires in flight, oldest first. */
  fires: MockPulseFire[];
  /** Each pulse's last fire that ended without a session (`last_fire` in `pulse_state.json`). */
  lastFires: Record<string, ScheduledRunOutcome>;
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
        modelTier: "small",
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
        model_tier: "anthropic/claude-haiku-4-5",
      },
    ],
    fires: [],
    lastFires: {},
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

/** The pulse a source label (`pulse:<name>`) names, or `null` for an action's. */
function pulseOf(source: string): string | null {
  return source.startsWith("pulse:") ? source.slice("pulse:".length) : null;
}

/**
 * The run of `source` (`pulse:<name>`, `action:<name>`) in flight now, as the
 * backend picks it: the pulse's newest fire, else the newest live session.
 * A fire's overlap is the oldest run of the pulse still in flight before it.
 */
function currentRun(state: MockState, source: string): ScheduledCurrentRun | null {
  const live = state.sessions.live.filter(
    (s) => s.category === "scheduled" && s.source_label === source,
  );
  const pulse = pulseOf(source);
  const fires = state.scheduled.fires.filter((f) => f.pulse === pulse);
  const fire = fires.at(-1);
  if (fire !== undefined) {
    const before: PulseOverlap[] = [
      ...fires.slice(0, -1).map((f) => ({
        previous_run_id: null,
        previous_fire_id: f.fireId,
        previous_started_at: f.startedAt,
      })),
      ...live.map((s) => ({
        previous_run_id: s.run_id,
        previous_fire_id: null,
        previous_started_at: s.started_at,
      })),
    ]
      .filter((run) => Date.parse(run.previous_started_at) <= Date.parse(fire.startedAt))
      .sort((a, b) => Date.parse(a.previous_started_at) - Date.parse(b.previous_started_at));
    return {
      fire_id: fire.fireId,
      phase: "script",
      started_at: fire.startedAt,
      address: null,
      run_id: null,
      overlap: before[0] ?? null,
    };
  }
  const run = live[0];
  return run === undefined
    ? null
    : {
        fire_id: null,
        phase: "session",
        started_at: run.started_at,
        address: run.address,
        run_id: run.run_id,
        overlap: run.overlap,
      };
}

/**
 * How the last run of `source` went, as the Scheduled view shows it: the
 * newer of its newest finished session and, for a pulse, its last fire that
 * ran no session.
 */
function lastOutcome(state: MockState, source: string): ScheduledRunOutcome | null {
  const run: SessionSummary | undefined = state.sessions.completed.find(
    (s) => s.category === "scheduled" && s.source_label === source,
  );
  const session: ScheduledRunOutcome | null =
    run === undefined
      ? null
      : {
          status: run.outcome ?? "completed",
          at: run.completed_at ?? run.started_at,
          error: run.error,
          run_id: run.run_id,
        };
  const pulse = pulseOf(source);
  const fire = pulse === null ? undefined : state.scheduled.lastFires[pulse];
  if (fire === undefined) return session;
  if (session === null) return fire;
  return Date.parse(fire.at) > Date.parse(session.at) ? fire : session;
}

/** Tell the agent's pages that a fire of `pulse` changed, and with what outcome when it ended with one. */
function announceFire(
  state: MockState,
  pulse: string,
  fireId: string,
  outcome: ScheduledRunOutcome | null,
): void {
  state.broadcast({
    type: "pulse_fire_changed",
    pulse,
    fire_id: fireId,
    current_run: currentRun(state, `pulse:${pulse}`),
    last_outcome: outcome,
  });
}

/**
 * Fire `pulse` now, in its task scripts, as the scheduler does when it is
 * due: its last run is now, and the fire is announced. `null` for a pulse
 * HEARTBEAT.yml doesn't have.
 */
export function startPulseFire(state: MockState, pulse: string): MockPulseFire | null {
  const declared = state.scheduled.pulses.find((p) => p.name === pulse);
  if (declared === undefined) return null;
  const startedAt = state.env.clock.iso();
  declared.lastRunAt = startedAt;
  const fire: MockPulseFire = {
    fireId: `fire-${String(state.env.nextId())}`,
    pulse,
    startedAt,
    stopping: false,
  };
  state.scheduled.fires.push(fire);
  announceFire(state, pulse, fire.fireId, null);
  return fire;
}

/**
 * End the fire `fireId` without a session: its outcome becomes its pulse's
 * last fire, and the end is announced with it. `false` for a fire that isn't
 * in flight.
 */
export function finishPulseFire(
  state: MockState,
  fireId: string,
  status: SessionRunStatus,
  error: string | null,
): boolean {
  const at = state.scheduled.fires.findIndex((f) => f.fireId === fireId);
  const fire = state.scheduled.fires[at];
  if (fire === undefined) return false;
  state.scheduled.fires.splice(at, 1);
  const outcome: ScheduledRunOutcome = { status, at: state.env.clock.iso(), error };
  state.scheduled.lastFires[fire.pulse] = outcome;
  announceFire(state, fire.pulse, fire.fireId, outcome);
  return true;
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
 * for a window that never opens: `next_active_moment` in `crates/proactivity/src/pulse/next_run.rs`.
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
 * will not: `next_run_at` in `crates/proactivity/src/pulse/next_run.rs`, the one calculation behind
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
    model_tier: pulse.modelTier ?? null,
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

/** How long a stopped fire takes to finish, as the backend's fire task ends its scripts. */
const STOP_MS = 300;

/**
 * `POST .../scheduled/pulses/{pulse}/fires/{fire_id}/stop`: answers `202
 * { pulse, fire_id }` and ends the fire as `cancelled` a moment later, or
 * `404` for a fire of that pulse that isn't in flight.
 */
function stopPulseFire(ctx: RouteContext): void {
  const { res, state } = ctx;
  const pulse = decodedParam(ctx, 0);
  const fireId = decodedParam(ctx, 1);
  const fire = state.scheduled.fires.find((f) => f.fireId === fireId && f.pulse === pulse);
  if (fire === undefined) {
    json(res, 404, { error: `This run of the pulse "${pulse}" isn't running any more.` });
    return;
  }
  if (!fire.stopping) {
    fire.stopping = true;
    state.env.after(STOP_MS, () => {
      finishPulseFire(state, fireId, "cancelled", null);
      noticeScheduleChange(ctx);
    });
  }
  json(res, 202, { pulse, fire_id: fireId });
}

const FIRE_STATUSES: readonly SessionRunStatus[] = ["completed", "failed", "cancelled"];

/**
 * Test control `POST /api/mock/pulse-fire` (`{ pulse }`): the pulse fires now
 * and runs its task scripts until `/api/mock/pulse-fire/finish` or a stop
 * ends it. Answers `{ fire_id }`.
 */
async function startPulseFireControl(ctx: RouteContext): Promise<void> {
  const pulse = stringField(await readJsonObject(ctx.req), "pulse") ?? "";
  const fire = startPulseFire(ctx.state, pulse);
  if (fire === null) {
    json(ctx.res, 404, { error: `mock: no pulse named "${pulse}"` });
    return;
  }
  noticeScheduleChange(ctx);
  json(ctx.res, 200, { fire_id: fire.fireId });
}

/**
 * Test control `POST /api/mock/pulse-fire/finish` (`{ fire_id, status, error? }`,
 * `status` `completed`, `failed` or `cancelled`): the fire's scripts end and
 * it runs no session, with that outcome.
 */
async function finishPulseFireControl(ctx: RouteContext): Promise<void> {
  const body = await readJsonObject(ctx.req);
  const fireId = stringField(body, "fire_id") ?? "";
  const status = FIRE_STATUSES.find((s) => s === body.status);
  if (status === undefined) {
    json(ctx.res, 422, { error: "mock: `status` must be completed, failed or cancelled" });
    return;
  }
  if (!finishPulseFire(ctx.state, fireId, status, stringField(body, "error") ?? null)) {
    json(ctx.res, 404, { error: `mock: no fire "${fireId}" in flight` });
    return;
  }
  noticeScheduleChange(ctx);
  json(ctx.res, 200, { ok: true });
}

/** The Scheduled view routes, in the unscoped `/api/...` spelling, and their test controls. */
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
    method: "POST",
    pattern: /^\/api\/scheduled\/pulses\/([^/]+)\/fires\/([^/]+)\/stop$/,
    handler: stopPulseFire,
  },
  { method: "POST", pattern: "/api/mock/pulse-fire", handler: startPulseFireControl },
  { method: "POST", pattern: "/api/mock/pulse-fire/finish", handler: finishPulseFireControl },
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
            problem: action.problem ?? null,
            current_run: currentRun(state, `action:${action.name}`),
          }),
        ),
      );
    },
  },
  { method: "DELETE", pattern: /^\/api\/scheduled\/actions\/([^/]+)$/, handler: cancelAction },
];
