/**
 * The instant a deterministic mock starts at, and returns to on reset: noon UTC
 * on a Saturday, after every sample message the chat data places "today".
 */
export const FIXED_START_MS = Date.UTC(2026, 2, 14, 12, 0, 0);

/**
 * The mock's one source of time. A fixed clock stands still until it is
 * advanced; a live one follows the wall clock plus whatever it was advanced by.
 */
export interface MockClock {
  /** Milliseconds since the epoch. */
  now: () => number;
  /** The current instant as an ISO 8601 string. */
  iso: () => string;
  /** The instant `ms` milliseconds from now, as an ISO 8601 string. */
  isoIn: (ms: number) => string;
  /** The instant `ms` milliseconds before now, as an ISO 8601 string. */
  isoAgo: (ms: number) => string;
  /**
   * `hour:minute` on the day `daysAgo` days back, as an ISO 8601 string. A fixed
   * clock reads the day in UTC so the result doesn't depend on the machine's
   * time zone; a live one reads it in local time.
   */
  dayAt: (daysAgo: number, hour: number, minute?: number) => string;
  /** The date `daysAgo` days back, as `YYYY-MM-DD`. */
  dateDaysAgo: (daysAgo: number) => string;
  /** Move the clock forward. */
  advance: (ms: number) => void;
  /** Milliseconds since the clock started or was last reset. */
  elapsedMs: () => number;
  /** Go back to where the clock started. */
  reset: () => void;
}

/** A clock that stands at `fixedStart` until advanced, or follows the wall clock when that is `null`. */
export function createClock(fixedStart: number | null): MockClock {
  let offset = 0;
  let startedAt = fixedStart ?? Date.now();
  const now = (): number => (fixedStart ?? Date.now()) + offset;
  const utc = fixedStart !== null;

  const daysBack = (daysAgo: number): Date => {
    const day = new Date(now());
    if (utc) day.setUTCDate(day.getUTCDate() - daysAgo);
    else day.setDate(day.getDate() - daysAgo);
    return day;
  };

  return {
    now,
    iso: () => new Date(now()).toISOString(),
    isoIn: (ms) => new Date(now() + ms).toISOString(),
    isoAgo: (ms) => new Date(now() - ms).toISOString(),
    dayAt: (daysAgo, hour, minute = 0) => {
      const day = daysBack(daysAgo);
      if (utc) day.setUTCHours(hour, minute, 0, 0);
      else day.setHours(hour, minute, 0, 0);
      return day.toISOString();
    },
    dateDaysAgo: (daysAgo) => daysBack(daysAgo).toISOString().slice(0, 10),
    advance: (ms) => {
      offset += ms;
    },
    elapsedMs: () => now() - startedAt,
    reset: () => {
      offset = 0;
      startedAt = fixedStart ?? Date.now();
    },
  };
}

/** A pending simulated delay answers this when the mock is reset before it ends. */
export class MockResetError extends Error {
  constructor() {
    super("the mock was reset");
    this.name = "MockResetError";
  }
}

export interface EnvOptions {
  /** A fixed clock, a stable scenario, and no delays unless `delayScale` says otherwise. */
  deterministic?: boolean;
  /** Multiplies every simulated duration: `1` is the natural pace and `0` is instant. */
  delayScale?: number;
}

/**
 * What the mock's behavior depends on beyond its data: the clock, how long
 * simulated work takes, and the numbers it hands out. Every timer the mock
 * starts goes through `after` or `sleep`, so a reset can cancel them all.
 */
export interface MockEnv {
  deterministic: boolean;
  clock: MockClock;
  /** The multiplier applied to simulated durations now. */
  delayScale: () => number;
  setDelayScale: (scale: number) => void;
  /** Run `action` after `ms` milliseconds of simulated time, unless cancelled or reset first. Returns its cancel. */
  after: (ms: number, action: () => void) => () => void;
  /** Resolve after `ms` milliseconds of simulated time, or reject with `MockResetError` if the mock is reset first. */
  sleep: (ms: number) => Promise<void>;
  /** How simulated time passes now: see `TimeMode`. */
  timeMode: () => TimeMode;
  /**
   * Change how simulated time passes. A timer already waiting moves with it:
   * into manual time with the delay it had left, or onto the wall clock for
   * what remains of its simulated delay.
   */
  setTimeMode: (mode: TimeMode) => void;
  /**
   * In manual time, move simulated time forward by `ms`. Each timer that comes
   * due runs in order, and what it starts gets to set its own timers before
   * the next one runs, so a timer set and due within the span runs too. The
   * clock moves with it.
   */
  advance: (ms: number) => Promise<TimeProgress>;
  /** In manual time, run the next timer that waits, moving simulated time and the clock to when it was due. */
  step: () => Promise<TimeProgress>;
  /** How many timers wait. */
  pendingTimers: () => number;
  /**
   * How far simulated turns may get, so a test can look at a running turn for
   * as long as it needs, however slowly the browser keeps up:
   * - `"reply"`: a turn writes its reply, then waits with the reply still
   *   streaming instead of ending.
   * - `"end"`: a turn runs through its steps, then waits before writing its reply.
   * - `"steps"`: a turn waits with its tool calls running, sending no more frames.
   * - `"none"`: turns run to their end.
   * Easing the hold lets the waiting turns carry on, in the order they waited.
   */
  holdTurns: (hold: TurnHold) => void;
  /** Run `action`, a turn's `stage`, now, or once the hold lets that stage through. Returns its cancel. */
  whenTurnReleased: (stage: TurnStage, action: () => void) => () => void;
  /** The next number of a sequence that starts at 1 again on reset, for ids that have to differ. */
  nextId: () => number;
  /** Cancel every pending timer and held turn, return the clock, the delays, the time mode, the hold and the sequence to where they started. */
  reset: () => void;
}

/**
 * How simulated time passes:
 * - `"scaled"`: on the wall clock, each delay multiplied by the delay scale.
 * - `"manual"`: only when a test moves it (`advance`, `step`). Nothing the mock
 *   simulates happens in between, however long the browser takes, so a test
 *   can look at one moment of a running turn for as long as it needs. The
 *   delay scale doesn't apply: a delay is its natural length.
 */
export type TimeMode = "scaled" | "manual";

/** What a move of manual time did. */
export interface TimeProgress {
  /** How many timers ran. */
  fired: number;
  /** How many still wait. */
  pending: number;
  /** Simulated milliseconds since manual time began. */
  elapsedMs: number;
}

/** How far simulated turns may get: see `MockEnv.holdTurns`. */
export type TurnHold = "none" | "reply" | "end" | "steps";

/**
 * The part of a turn a hold can stop at: delivering its tool results, writing
 * its reply, or finishing once the reply is written.
 */
export type TurnStage = "results" | "end" | "finish";

/** How much of a turn each hold stops, and the hold each stage needs to be stopped: a later stage stops at more holds. */
const HOLD_REACH: Record<TurnHold, number> = { none: 0, reply: 1, end: 2, steps: 3 };
const STAGE_REACH: Record<TurnStage, number> = { finish: 1, end: 2, results: 3 };

/** Whether `hold` keeps a turn from carrying out `stage`. */
function holds(hold: TurnHold, stage: TurnStage): boolean {
  return HOLD_REACH[hold] >= STAGE_REACH[stage];
}

/** A timer waiting for its moment of simulated time. */
interface Timer {
  /** When it runs: the wall-clock instant in scaled time, the simulated one in manual time. */
  due: number;
  /** The order it was set in, which breaks ties between timers due together. */
  order: number;
  /** In scaled time, the wall-clock timer that runs it. */
  handle: NodeJS.Timeout | undefined;
  fire: () => void;
  /** What a reset does to it: a `sleep` rejects, an `after` is dropped. */
  abort: () => void;
}

/**
 * How many event-loop turns a move of manual time gives a fired timer's work
 * to set its next timers. The mock's work is in memory, so a chain settles in
 * a few turns; the limit only stops a timer that keeps setting another at
 * once from holding the move forever.
 */
const SETTLE_TURN_LIMIT = 50;

const nextTurn = (): Promise<void> =>
  new Promise((resolve) => {
    setImmediate(resolve);
  });

export function createMockEnv(options: EnvOptions = {}): MockEnv {
  const deterministic = options.deterministic ?? false;
  const initialScale = options.delayScale ?? (deterministic ? 0 : 1);
  const clock = createClock(deterministic ? FIXED_START_MS : null);
  let scale = initialScale;
  let sequence = 0;
  let timerOrder = 0;
  let mode: TimeMode = "scaled";
  /** Simulated milliseconds since manual time began. */
  let simulatedMs = 0;
  /** Bumped whenever a timer is set or goes, so `settle` can tell when a fired timer's work has stopped setting more. */
  let timersChanged = 0;
  const timers = new Set<Timer>();
  let turnHold: TurnHold = "none";
  const heldTurns = new Set<{ stage: TurnStage; action: () => void }>();

  const run = (timer: Timer): void => {
    timers.delete(timer);
    timersChanged += 1;
    timer.fire();
  };

  /** Put `timer` on the wall clock, `remainingMs` of simulated time from now. */
  const schedule = (timer: Timer, remainingMs: number): void => {
    const delay = remainingMs * scale;
    timer.due = Date.now() + delay;
    timer.handle = setTimeout(() => {
      run(timer);
    }, delay);
  };

  const addTimer = (ms: number, fire: () => void, abort: () => void): Timer => {
    const timer: Timer = { due: 0, order: timerOrder++, handle: undefined, fire, abort };
    if (mode === "manual") timer.due = simulatedMs + ms;
    else schedule(timer, ms);
    timers.add(timer);
    timersChanged += 1;
    return timer;
  };

  const removeTimer = (timer: Timer): void => {
    if (timer.handle !== undefined) clearTimeout(timer.handle);
    if (timers.delete(timer)) timersChanged += 1;
  };

  /** The manual-time timer that runs next, if one is due by `limit`. */
  const nextDue = (limit: number): Timer | undefined => {
    let next: Timer | undefined;
    for (const timer of timers) {
      if (timer.due > limit) continue;
      const earlier =
        next === undefined ||
        timer.due < next.due ||
        (timer.due === next.due && timer.order < next.order);
      if (earlier) next = timer;
    }
    return next;
  };

  /** Let a fired timer's work run until it stops setting or clearing timers. */
  const settle = async (): Promise<void> => {
    let quietTurns = 0;
    for (let turn = 0; turn < SETTLE_TURN_LIMIT && quietTurns < 2; turn++) {
      const before = timersChanged;
      await nextTurn();
      quietTurns = timersChanged === before ? quietTurns + 1 : 0;
    }
  };

  /** Move simulated time, and the clock with it, to `target`. */
  const moveTo = (target: number): void => {
    clock.advance(target - simulatedMs);
    simulatedMs = target;
  };

  const progress = (fired: number): TimeProgress => ({
    fired,
    pending: timers.size,
    elapsedMs: simulatedMs,
  });

  const requireManual = (what: string): void => {
    if (mode !== "manual") throw new Error(`mock: ${what} needs manual time; set it first`);
  };

  return {
    deterministic,
    clock,
    delayScale: () => scale,
    setDelayScale: (next) => {
      scale = next;
    },
    after: (ms, action) => {
      const timer = addTimer(ms, action, () => undefined);
      return () => {
        removeTimer(timer);
      };
    },
    sleep: (ms) =>
      new Promise<void>((resolve, reject) => {
        addTimer(ms, resolve, () => {
          reject(new MockResetError());
        });
      }),
    timeMode: () => mode,
    setTimeMode: (next) => {
      if (next === mode) return;
      if (next === "manual") {
        // A scaled timer keeps the share of its simulated delay it hadn't waited yet.
        simulatedMs = 0;
        const now = Date.now();
        for (const timer of timers) {
          if (timer.handle !== undefined) clearTimeout(timer.handle);
          timer.handle = undefined;
          timer.due = scale > 0 ? Math.max(0, timer.due - now) / scale : 0;
        }
      } else {
        for (const timer of timers) schedule(timer, Math.max(0, timer.due - simulatedMs));
      }
      mode = next;
    },
    advance: async (ms) => {
      requireManual("advancing time");
      const target = simulatedMs + ms;
      let fired = 0;
      for (let next = nextDue(target); next !== undefined; next = nextDue(target)) {
        moveTo(next.due);
        run(next);
        fired += 1;
        await settle();
      }
      moveTo(target);
      return progress(fired);
    },
    step: async () => {
      requireManual("stepping time");
      const next = nextDue(Number.POSITIVE_INFINITY);
      if (next === undefined) return progress(0);
      moveTo(Math.max(simulatedMs, next.due));
      run(next);
      await settle();
      return progress(1);
    },
    pendingTimers: () => timers.size,
    holdTurns: (hold) => {
      turnHold = hold;
      for (const entry of [...heldTurns]) {
        if (holds(turnHold, entry.stage)) continue;
        heldTurns.delete(entry);
        entry.action();
      }
    },
    whenTurnReleased: (stage, action) => {
      if (!holds(turnHold, stage)) {
        action();
        return () => undefined;
      }
      const entry = { stage, action };
      heldTurns.add(entry);
      return () => {
        heldTurns.delete(entry);
      };
    },
    nextId: () => ++sequence,
    reset: () => {
      for (const timer of [...timers]) {
        if (timer.handle !== undefined) clearTimeout(timer.handle);
        timer.abort();
      }
      timers.clear();
      timersChanged += 1;
      mode = "scaled";
      simulatedMs = 0;
      heldTurns.clear();
      turnHold = "none";
      scale = initialScale;
      sequence = 0;
      clock.reset();
    },
  };
}
