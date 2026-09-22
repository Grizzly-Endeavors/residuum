import { describe, expect, it } from "vitest";
import { relativeTime } from "./time";

const NOW = Date.parse("2026-09-22T12:00:00Z");
const ago = (seconds: number): Date => new Date(NOW - seconds * 1000);

describe("relativeTime", () => {
  it.each<[number, string]>([
    [0, "just now"],
    [44, "just now"],
    [60, "1m ago"],
    [59 * 60, "59m ago"],
    [60 * 60, "1h ago"],
    [23 * 3600, "23h ago"],
    [24 * 3600, "1d ago"],
    [10 * 86400, "10d ago"],
  ])("%is ago reads %s", (seconds, text) => {
    expect(relativeTime(ago(seconds), NOW)).toBe(text);
  });

  it("treats future times as just now", () => {
    expect(relativeTime(ago(-600), NOW)).toBe("just now");
  });

  it("accepts ISO strings and returns empty for unparseable ones", () => {
    expect(relativeTime("2026-09-22T11:00:00Z", NOW)).toBe("1h ago");
    expect(relativeTime("not a date", NOW)).toBe("");
  });
});
