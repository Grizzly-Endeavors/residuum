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
  /**
   * How far simulated turns may get, so a test can look at a running turn for
   * as long as it needs, however slowly the browser keeps up:
   * - `"end"`: a turn runs through its steps, then waits instead of ending.
   * - `"steps"`: a turn waits with its tool calls running, sending no more
   *   frames, so a page that connects now sees nothing until the hold eases.
   * - `"none"`: turns run to their end.
   * Easing the hold lets the waiting turns carry on, in the order they waited.
   */
  holdTurns: (hold: TurnHold) => void;
  /** Run `action`, a turn's `stage`, now, or once the hold lets that stage through. Returns its cancel. */
  whenTurnReleased: (stage: TurnStage, action: () => void) => () => void;
  /** The next number of a sequence that starts at 1 again on reset, for ids that have to differ. */
  nextId: () => number;
  /** Cancel every pending timer and held turn, return the clock, the delays, the hold and the sequence to where they started. */
  reset: () => void;
}

/** How far simulated turns may get: see `MockEnv.holdTurns`. */
export type TurnHold = "none" | "end" | "steps";

/** The part of a turn a hold can stop at: delivering its tool results, or ending it. */
export type TurnStage = "results" | "end";

/** Whether `hold` keeps a turn from carrying out `stage`. */
function holds(hold: TurnHold, stage: TurnStage): boolean {
  return stage === "end" ? hold !== "none" : hold === "steps";
}

export function createMockEnv(options: EnvOptions = {}): MockEnv {
  const deterministic = options.deterministic ?? false;
  const initialScale = options.delayScale ?? (deterministic ? 0 : 1);
  const clock = createClock(deterministic ? FIXED_START_MS : null);
  let scale = initialScale;
  let sequence = 0;
  const pending = new Set<{ timer: NodeJS.Timeout; abort: () => void }>();
  let turnHold: TurnHold = "none";
  const heldTurns = new Set<{ stage: TurnStage; action: () => void }>();

  return {
    deterministic,
    clock,
    delayScale: () => scale,
    setDelayScale: (next) => {
      scale = next;
    },
    after: (ms, action) => {
      const entry = {
        timer: setTimeout(() => {
          pending.delete(entry);
          action();
        }, ms * scale),
        abort: () => undefined,
      };
      pending.add(entry);
      return () => {
        clearTimeout(entry.timer);
        pending.delete(entry);
      };
    },
    sleep: (ms) =>
      new Promise<void>((resolve, reject) => {
        const entry = {
          timer: setTimeout(() => {
            pending.delete(entry);
            resolve();
          }, ms * scale),
          abort: () => {
            reject(new MockResetError());
          },
        };
        pending.add(entry);
      }),
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
      for (const entry of [...pending]) {
        clearTimeout(entry.timer);
        entry.abort();
      }
      pending.clear();
      heldTurns.clear();
      turnHold = "none";
      scale = initialScale;
      sequence = 0;
      clock.reset();
    },
  };
}
