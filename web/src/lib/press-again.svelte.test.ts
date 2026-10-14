import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PressAgain } from "./press-again.svelte";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("PressAgain", () => {
  it("arms on the first press and goes through on the second", () => {
    const key = new PressAgain();
    expect(key.press()).toBe(false);
    expect(key.armed).toBe(true);
    expect(key.press()).toBe(true);
    expect(key.armed).toBe(false);
  });

  it("puts itself back once the window runs out", () => {
    const key = new PressAgain(2000);
    key.press();
    vi.advanceTimersByTime(1999);
    expect(key.armed).toBe(true);
    vi.advanceTimersByTime(1);
    expect(key.armed).toBe(false);
    // A press after the window starts a new pair.
    expect(key.press()).toBe(false);
    expect(key.armed).toBe(true);
  });

  it("counts the window from the first press, not a later one", () => {
    const key = new PressAgain(2000);
    key.press();
    vi.advanceTimersByTime(1500);
    expect(key.press()).toBe(true);
    // The finished pair left nothing to time out.
    expect(vi.getTimerCount()).toBe(0);
  });

  it("is disarmed on demand", () => {
    const key = new PressAgain();
    key.press();
    key.disarm();
    expect(key.armed).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    expect(key.press()).toBe(false);
  });
});
