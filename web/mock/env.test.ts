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
});
