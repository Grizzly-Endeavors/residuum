import { describe, expect, it } from "vitest";
import { createClock, createMockEnv, FIXED_START_MS, MockResetError } from "./env";

describe("the clock", () => {
  it("stands still at its start until advanced, and back again after a reset", () => {
    const clock = createClock(FIXED_START_MS);
    expect(clock.iso()).toBe("2026-03-14T12:00:00.000Z");
    expect(clock.iso()).toBe("2026-03-14T12:00:00.000Z");
    clock.advance(90_000);
    expect(clock.iso()).toBe("2026-03-14T12:01:30.000Z");
    expect(clock.elapsedMs()).toBe(90_000);
    clock.reset();
    expect(clock.iso()).toBe("2026-03-14T12:00:00.000Z");
    expect(clock.elapsedMs()).toBe(0);
  });

  it("places times relative to now", () => {
    const clock = createClock(FIXED_START_MS);
    expect(clock.isoAgo(60_000)).toBe("2026-03-14T11:59:00.000Z");
    expect(clock.isoIn(3_600_000)).toBe("2026-03-14T13:00:00.000Z");
    expect(clock.dayAt(2, 9, 12)).toBe("2026-03-12T09:12:00.000Z");
    expect(clock.dayAt(0, 10)).toBe("2026-03-14T10:00:00.000Z");
    expect(clock.dateDaysAgo(8)).toBe("2026-03-06");
  });

  it("follows the wall clock when it isn't fixed, and can still be advanced", () => {
    const clock = createClock(null);
    const before = Date.now();
    const at = clock.now();
    expect(at).toBeGreaterThanOrEqual(before);
    expect(at).toBeLessThan(before + 1000);
    clock.advance(86_400_000);
    expect(clock.now() - Date.now()).toBeGreaterThan(86_000_000);
  });
});

describe("the mock environment", () => {
  it("is live by default: the wall clock and the natural pace", () => {
    const env = createMockEnv();
    expect(env.deterministic).toBe(false);
    expect(env.delayScale()).toBe(1);
  });

  it("is fixed, with no delays, when deterministic", () => {
    const env = createMockEnv({ deterministic: true });
    expect(env.clock.now()).toBe(FIXED_START_MS);
    expect(env.delayScale()).toBe(0);
    expect(createMockEnv({ deterministic: true, delayScale: 0.5 }).delayScale()).toBe(0.5);
  });

  it("runs timers in the order they were set when every delay is zero", async () => {
    const env = createMockEnv({ deterministic: true });
    const ran: string[] = [];
    env.after(4000, () => ran.push("late"));
    env.after(300, () => ran.push("early"));
    await env.sleep(10_000);
    expect(ran).toEqual(["late", "early"]);
  });

  it("scales the delay it waits", async () => {
    const env = createMockEnv({ delayScale: 0.01 });
    const start = Date.now();
    await env.sleep(2000);
    expect(Date.now() - start).toBeLessThan(1000);
  });

  it("cancels a timer, and every pending timer on reset", async () => {
    const env = createMockEnv({ delayScale: 1 });
    const ran: string[] = [];
    env.after(20, () => ran.push("cancelled"))();
    env.after(20, () => ran.push("reset"));
    env.reset();
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(ran).toEqual([]);
  });

  it("rejects a sleep the reset cut short", async () => {
    const env = createMockEnv({ delayScale: 1 });
    const waiting = env.sleep(10_000);
    env.reset();
    await expect(waiting).rejects.toBeInstanceOf(MockResetError);
  });

  it("restarts its sequence, clock and delays on reset", () => {
    const env = createMockEnv({ deterministic: true });
    expect([env.nextId(), env.nextId()]).toEqual([1, 2]);
    env.clock.advance(5000);
    env.setDelayScale(1);
    env.reset();
    expect(env.nextId()).toBe(1);
    expect(env.clock.now()).toBe(FIXED_START_MS);
    expect(env.delayScale()).toBe(0);
  });

  it("ends a turn at once unless turn ends are held, then when the hold lifts", () => {
    const env = createMockEnv({ deterministic: true });
    const ended: string[] = [];
    env.whenTurnReleased("end", () => ended.push("free"));
    env.holdTurns("end");
    env.whenTurnReleased("end", () => ended.push("first"));
    env.whenTurnReleased("end", () => ended.push("second"));
    expect(ended).toEqual(["free"]);
    env.holdTurns("none");
    expect(ended).toEqual(["free", "first", "second"]);
  });

  it('holds a turn\'s results as well as its end at "steps", and lets the results out first as the hold eases', () => {
    const env = createMockEnv({ deterministic: true });
    const stages: string[] = [];
    env.holdTurns("steps");
    env.whenTurnReleased("results", () => stages.push("results"));
    env.whenTurnReleased("end", () => stages.push("end"));
    expect(stages).toEqual([]);
    env.holdTurns("end");
    expect(stages).toEqual(["results"]);
    env.whenTurnReleased("results", () => stages.push("late results"));
    expect(stages).toEqual(["results", "late results"]);
    env.holdTurns("none");
    expect(stages).toEqual(["results", "late results", "end"]);
  });

  it("drops a held turn end that was cancelled, or cut short by a reset", () => {
    const env = createMockEnv({ deterministic: true });
    const ended: string[] = [];
    env.holdTurns("end");
    env.whenTurnReleased("end", () => ended.push("cancelled"))();
    env.whenTurnReleased("end", () => ended.push("reset"));
    env.reset();
    env.holdTurns("none");
    expect(ended).toEqual([]);
    env.whenTurnReleased("end", () => ended.push("after reset"));
    expect(ended).toEqual(["after reset"]);
  });
});

describe("manual time", () => {
  function manualEnv(): ReturnType<typeof createMockEnv> {
    const env = createMockEnv({ deterministic: true });
    env.setTimeMode("manual");
    return env;
  }

  it("runs nothing until time is moved, then each timer in due order as the clock passes it", async () => {
    const env = manualEnv();
    const ran: string[] = [];
    env.after(300, () => ran.push(`b at ${String(env.clock.elapsedMs())}`));
    env.after(100, () => ran.push(`a at ${String(env.clock.elapsedMs())}`));
    env.after(900, () => ran.push("c"));
    // Turns of the event loop pass; simulated time doesn't.
    await new Promise((resolve) => setImmediate(resolve));
    expect(ran).toEqual([]);

    expect(await env.advance(500)).toEqual({ fired: 2, pending: 1, elapsedMs: 500 });
    expect(ran).toEqual(["a at 100", "b at 300"]);
    expect(env.clock.elapsedMs()).toBe(500);
  });

  it("runs a timer that a fired timer's work sets within the span", async () => {
    const env = manualEnv();
    const ran: string[] = [];
    const turn = async (): Promise<void> => {
      await env.sleep(100);
      ran.push("first step");
      await env.sleep(100);
      ran.push("second step");
      await env.sleep(1000);
      ran.push("end");
    };
    const running = turn();
    expect((await env.advance(250)).fired).toBe(2);
    expect(ran).toEqual(["first step", "second step"]);
    expect(await env.advance(1000)).toMatchObject({ fired: 1, pending: 0 });
    await running;
    expect(ran).toEqual(["first step", "second step", "end"]);
  });

  it("steps to the next timer however far ahead it is, and reports when none waits", async () => {
    const env = manualEnv();
    const ran: string[] = [];
    env.after(60_000, () => ran.push("late"));
    expect(await env.step()).toEqual({ fired: 1, pending: 0, elapsedMs: 60_000 });
    expect(ran).toEqual(["late"]);
    expect(await env.step()).toEqual({ fired: 0, pending: 0, elapsedMs: 60_000 });
  });

  it("breaks a tie between timers due together by the order they were set", async () => {
    const env = manualEnv();
    const ran: string[] = [];
    env.after(50, () => ran.push("first"));
    env.after(50, () => ran.push("second"));
    await env.advance(50);
    expect(ran).toEqual(["first", "second"]);
  });

  it("ignores the delay scale, and refuses to move while time is scaled", async () => {
    const env = createMockEnv({ deterministic: true, delayScale: 8 });
    await expect(env.advance(10)).rejects.toThrow(/manual time/);
    env.setTimeMode("manual");
    const ran: string[] = [];
    env.after(100, () => ran.push("natural length"));
    await env.advance(100);
    expect(ran).toEqual(["natural length"]);
  });

  it("drops a cancelled timer", async () => {
    const env = manualEnv();
    const ran: string[] = [];
    env.after(10, () => ran.push("cancelled"))();
    expect(env.pendingTimers()).toBe(0);
    await env.advance(100);
    expect(ran).toEqual([]);
  });

  it("carries a waiting timer between modes with the simulated delay it had left", async () => {
    const env = createMockEnv({ deterministic: true, delayScale: 0 });
    const ran: string[] = [];
    env.setTimeMode("manual");
    env.after(400, () => ran.push("set in manual"));
    await env.advance(100);
    env.setTimeMode("scaled");
    // At scale 0 the remaining 300ms is no wait at all.
    await env.sleep(0);
    await new Promise((resolve) => setImmediate(resolve));
    expect(ran).toEqual(["set in manual"]);
  });

  it("goes back to scaled time on reset, rejecting the sleeps that waited", async () => {
    const env = manualEnv();
    const waiting = env.sleep(1000);
    await env.advance(500);
    env.reset();
    await expect(waiting).rejects.toBeInstanceOf(MockResetError);
    expect(env.timeMode()).toBe("scaled");
    expect(env.pendingTimers()).toBe(0);
    expect(env.clock.now()).toBe(FIXED_START_MS);
  });
});
