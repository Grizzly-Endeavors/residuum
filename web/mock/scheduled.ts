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

const HOUR_MS = 3_600_000;

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

function nextFireAt(pulse: MockPulse, clock: MockClock): string | null {
  const duration = pulse.schedule === null ? null : scheduleMs(pulse.schedule);
  if (!pulse.enabled || duration === null) return null;
  if (pulse.lastRunAt === null) return clock.iso();
  return new Date(Date.parse(pulse.lastRunAt) + duration).toISOString();
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
