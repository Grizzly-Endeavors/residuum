import { describe, expect, it } from "vitest";
import { formatElapsed, formatTokenCount } from "./format-usage";

describe("formatElapsed", () => {
  it("shows plain seconds under a minute", () => {
    expect(formatElapsed(0)).toBe("0s");
    expect(formatElapsed(999)).toBe("0s");
    expect(formatElapsed(1000)).toBe("1s");
    expect(formatElapsed(45_000)).toBe("45s");
  });

  it("shows minutes and seconds under an hour", () => {
    expect(formatElapsed(72_000)).toBe("1m 12s");
    expect(formatElapsed(60_000)).toBe("1m 00s");
    expect(formatElapsed(59 * 60_000 + 59_000)).toBe("59m 59s");
  });

  it("shows hours and minutes at an hour or beyond", () => {
    expect(formatElapsed(60 * 60_000)).toBe("1h 00m");
    expect(formatElapsed(60 * 60_000 + 3 * 60_000 + 12_000)).toBe("1h 03m");
  });

  it("never goes negative for a clock that hasn't ticked yet", () => {
    expect(formatElapsed(-50)).toBe("0s");
  });
});

describe("formatTokenCount", () => {
  it("shows the plain number under 1000", () => {
    expect(formatTokenCount(0)).toBe("0");
    expect(formatTokenCount(950)).toBe("950");
  });

  it("shows one decimal in the thousands, trimming a bare .0", () => {
    expect(formatTokenCount(4300)).toBe("4.3k");
    expect(formatTokenCount(1000)).toBe("1k");
    expect(formatTokenCount(1050)).toBe("1.1k");
  });

  it("shows one decimal in the millions, trimming a bare .0", () => {
    expect(formatTokenCount(2_500_000)).toBe("2.5M");
    expect(formatTokenCount(1_000_000)).toBe("1M");
  });
});
