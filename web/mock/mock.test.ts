import { describe, expect, it } from "vitest";
import { DETERMINISTIC_ARTIFACTS_PORT, readMockOptions } from "./mock";

describe("readMockOptions", () => {
  it("is live by default: the natural pace, and any free port for artifacts", () => {
    expect(readMockOptions({})).toEqual({
      setup: false,
      deterministic: false,
      delayScale: 1,
      artifactsPort: 0,
    });
  });

  it("is deterministic with MOCK_DETERMINISTIC=1: no delays, and a fixed artifacts port", () => {
    expect(readMockOptions({ MOCK_DETERMINISTIC: "1" })).toEqual({
      setup: false,
      deterministic: true,
      delayScale: 0,
      artifactsPort: DETERMINISTIC_ARTIFACTS_PORT,
    });
  });

  it("only takes MOCK_DETERMINISTIC=1 for deterministic", () => {
    for (const value of ["0", "true", "", "yes"]) {
      expect(readMockOptions({ MOCK_DETERMINISTIC: value }).deterministic).toBe(false);
    }
  });

  it("lets the delay scale and the port override the mode's", () => {
    expect(
      readMockOptions({
        MOCK_DETERMINISTIC: "1",
        MOCK_DELAY_SCALE: "0.25",
        MOCK_ARTIFACTS_PORT: "6123",
      }),
    ).toMatchObject({ deterministic: true, delayScale: 0.25, artifactsPort: 6123 });
    expect(readMockOptions({ MOCK_DELAY_SCALE: "0" }).delayScale).toBe(0);
  });

  it("ignores a delay scale or port that isn't a non-negative number", () => {
    for (const value of ["fast", "-1", "", "NaN", "Infinity"]) {
      expect(
        readMockOptions({ MOCK_DELAY_SCALE: value, MOCK_ARTIFACTS_PORT: value }),
      ).toMatchObject({ delayScale: 1, artifactsPort: 0 });
    }
  });

  it("starts with no agents for VITE_MOCK_SETUP=1", () => {
    expect(readMockOptions({ VITE_MOCK_SETUP: "1" }).setup).toBe(true);
    expect(readMockOptions({ VITE_MOCK_SETUP: "0" }).setup).toBe(false);
  });
});
