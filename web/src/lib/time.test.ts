import { afterEach, describe, expect, it, vi } from "vitest";
import { localDay, localTimestamp, messageTimeLabel, parseTimestamp, relativeTime } from "./time";

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

/** Run the test with the clock of `zone`, the way a reader in it sees the page. */
function inZone(zone: string): void {
  vi.stubEnv("TZ", zone);
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("localTimestamp", () => {
  it("is the reader's wall clock in the shape history uses, not UTC", () => {
    inZone("America/Los_Angeles");
    // 06:30 UTC on the 10th is 23:30 on the 9th in Los Angeles.
    expect(localTimestamp(new Date("2026-10-10T06:30:12Z"))).toBe("2026-10-09T23:30:12");
    inZone("Pacific/Auckland");
    // 11:30 UTC on the 9th is 00:30 on the 10th in Auckland.
    expect(localTimestamp(new Date("2026-10-09T11:30:12Z"))).toBe("2026-10-10T00:30:12");
  });

  it("pads every field", () => {
    expect(localTimestamp(new Date(2026, 0, 2, 3, 4, 5))).toBe("2026-01-02T03:04:05");
  });
});

describe("localDay", () => {
  it("takes the day a zoneless timestamp names, as history writes them", () => {
    expect(localDay("2026-10-09T23:30:12")).toBe("2026-10-09");
    expect(localDay("2026-10-09T23:30")).toBe("2026-10-09");
    expect(localDay("2026-10-09")).toBe("2026-10-09");
  });

  it("moves a timestamp that names a zone to the reader's day", () => {
    inZone("America/Los_Angeles");
    expect(localDay("2026-10-10T06:30:00Z")).toBe("2026-10-09");
    expect(localDay("2026-10-10T08:30:00+02:00")).toBe("2026-10-09");
    inZone("Pacific/Auckland");
    expect(localDay("2026-10-09T11:30:00.000Z")).toBe("2026-10-10");
  });
});

describe("messageTimeLabel", () => {
  const clock = (at: Date): string =>
    new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(at);

  it("is the time of day for a message from today", () => {
    const now = new Date(2026, 9, 9, 18, 0).getTime();
    expect(messageTimeLabel("2026-10-09T10:05:00", now)).toBe(clock(new Date(2026, 9, 9, 10, 5)));
  });

  it("adds the date for another day, and the year for another year", () => {
    const now = new Date(2026, 9, 9, 18, 0).getTime();
    const yesterday = messageTimeLabel("2026-10-08T10:05:00", now);
    expect(yesterday).toContain(clock(new Date(2026, 9, 8, 10, 5)));
    expect(yesterday).toMatch(/8/);
    expect(yesterday).not.toContain("2026");
    expect(messageTimeLabel("2025-12-31T23:59:00", now)).toContain("2025");
  });

  it("reads today on the reader's clock, not UTC's", () => {
    inZone("America/Los_Angeles");
    // 23:30 on the 9th in Los Angeles, which is already the 10th in UTC.
    const now = new Date("2026-10-10T06:45:00Z").getTime();
    expect(messageTimeLabel("2026-10-09T23:30:00", now)).toBe(clock(new Date(2026, 9, 9, 23, 30)));
  });

  it("reads a long fraction of a second, which history carries", () => {
    const now = new Date(2026, 9, 9, 18, 0).getTime();
    expect(messageTimeLabel("2026-10-09T10:05:30.123456789", now)).toBe(
      clock(new Date(2026, 9, 9, 10, 5)),
    );
  });

  it("is empty for text that names no moment", () => {
    expect(messageTimeLabel("sometime")).toBe("");
  });
});

describe("parseTimestamp", () => {
  it("gives the moment, or null", () => {
    expect(parseTimestamp("2026-10-09T10:05:00Z")?.toISOString()).toBe("2026-10-09T10:05:00.000Z");
    expect(parseTimestamp("nope")).toBeNull();
  });
});
