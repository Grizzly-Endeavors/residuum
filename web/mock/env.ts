export const FIXED_START_MS = Date.UTC(2026, 2, 14, 12, 0, 0);

export interface MockClock {
  now: () => number;
  iso: () => string;
  isoIn: (ms: number) => string;
  isoAgo: (ms: number) => string;
  dayAt: (daysAgo: number, hour: number, minute?: number) => string;
  dateDaysAgo: (daysAgo: number) => string;
  advance: (ms: number) => void;
  elapsedMs: () => number;
  reset: () => void;
}

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

export class MockResetError extends Error {
  constructor() {
    super("the mock was reset");
    this.name = "MockResetError";
  }
}

export interface EnvOptions {
  deterministic?: boolean;
  delayScale?: number;
}

export interface MockEnv {
  deterministic: boolean;
  clock: MockClock;
  delayScale: () => number;
  setDelayScale: (scale: number) => void;
  after: (ms: number, action: () => void) => () => void;
  sleep: (ms: number) => Promise<void>;
  timeMode: () => TimeMode;
  setTimeMode: (mode: TimeMode) => void;
  advance: (ms: number) => Promise<TimeProgress>;
  step: () => Promise<TimeProgress>;
  pendingTimers: () => number;
  nextTimerIn: () => number | null;
  holdTurns: (hold: TurnHold) => void;
  whenTurnReleased: (stage: TurnStage, action: () => void) => () => void;
  nextId: () => number;
  reset: () => void;
}

export type TimeMode = "scaled" | "manual";

export interface TimeProgress {
  fired: number;
  pending: number;
  elapsedMs: number;
}

export type TurnHold = "none" | "reply" | "end" | "steps";

export type TurnStage = "results" | "end" | "finish";

const HOLD_REACH: Record<TurnHold, number> = { none: 0, reply: 1, end: 2, steps: 3 };
const STAGE_REACH: Record<TurnStage, number> = { finish: 1, end: 2, results: 3 };

function holds(hold: TurnHold, stage: TurnStage): boolean {
  return HOLD_REACH[hold] >= STAGE_REACH[stage];
}

interface Timer {
  due: number;
  order: number;
  handle: NodeJS.Timeout | undefined;
  fire: () => void;
  abort: () => void;
}

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
  let simulatedMs = 0;
  let timersChanged = 0;
  const timers = new Set<Timer>();
  let turnHold: TurnHold = "none";
  const heldTurns = new Set<{ stage: TurnStage; action: () => void }>();

  const run = (timer: Timer): void => {
    timers.delete(timer);
    timersChanged += 1;
    timer.fire();
  };

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

  const settle = async (): Promise<void> => {
    let quietTurns = 0;
    for (let turn = 0; turn < SETTLE_TURN_LIMIT && quietTurns < 2; turn++) {
      const before = timersChanged;
      await nextTurn();
      quietTurns = timersChanged === before ? quietTurns + 1 : 0;
    }
  };

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
    nextTimerIn: () => {
      if (mode !== "manual") return null;
      const next = nextDue(Number.POSITIVE_INFINITY);
      return next === undefined ? null : Math.max(0, next.due - simulatedMs);
    },
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
