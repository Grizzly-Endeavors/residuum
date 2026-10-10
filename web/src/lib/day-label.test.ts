import { describe, expect, it } from "vitest";
import { dayLabel } from "./day-label";

// Wednesday 8 October 2026, midday.
const NOW = new Date(2026, 9, 8, 12, 0).getTime();

describe("dayLabel", () => {
  it("names today and yesterday", () => {
    expect(dayLabel("2026-10-08", NOW)).toBe("Today");
    expect(dayLabel("2026-10-08T00:01", NOW)).toBe("Today");
    expect(dayLabel("2026-10-07", NOW)).toBe("Yesterday");
  });

  it("gives an older day in the same year as month and day", () => {
    expect(dayLabel("2026-10-06", NOW)).toBe(
      new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(
        new Date(2026, 9, 6),
      ),
    );
  });

  it("adds the year once it isn't this one", () => {
    expect(dayLabel("2025-12-31", NOW)).toContain("2025");
  });

  it("reads a timestamp and a bare date the same way", () => {
    expect(dayLabel("2026-10-06T23:59", NOW)).toBe(dayLabel("2026-10-06", NOW));
  });

  it("counts calendar days across midnight, not hours", () => {
    const justAfterMidnight = new Date(2026, 9, 8, 0, 5).getTime();
    expect(dayLabel("2026-10-07T23:50", justAfterMidnight)).toBe("Yesterday");
  });

  it("counts calendar days across a change of clocks", () => {
    // Clocks went back on 2026-10-25 in most of Europe and 2026-11-01 in the US.
    expect(dayLabel("2026-11-01", new Date(2026, 10, 2, 9, 0).getTime())).toBe("Yesterday");
  });

  it("leaves a day that hasn't happened yet by its date", () => {
    expect(dayLabel("2026-10-09", NOW)).not.toBe("Today");
    expect(dayLabel("2026-10-09", NOW)).not.toBe("Yesterday");
  });

  it("falls back to the value itself when it names no day", () => {
    expect(dayLabel("unknown", NOW)).toBe("unknown");
  });
});
