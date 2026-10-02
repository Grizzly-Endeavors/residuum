import { describe, expect, it } from "vitest";
import { isTimeZoneName, timeZoneChoices } from "./time-zones";

describe("timeZoneChoices", () => {
  it("lists UTC on its own and groups the rest by area", () => {
    const { ungrouped, groups } = timeZoneChoices("");
    expect(ungrouped.map((choice) => choice.value)[0]).toBe("UTC");
    const america = groups.find((group) => group.label === "America");
    expect(america?.options.map((choice) => choice.value)).toContain("America/New_York");
  });

  it("keeps a saved name the list doesn't know", () => {
    const { ungrouped } = timeZoneChoices("Mars/Olympus");
    expect(ungrouped[0]).toEqual({ value: "Mars/Olympus", label: "Mars/Olympus" });
  });

  it("does not duplicate a known name", () => {
    const { ungrouped, groups } = timeZoneChoices("Europe/Berlin");
    expect(ungrouped.map((choice) => choice.value)).not.toContain("Europe/Berlin");
    const europe = groups.find((group) => group.label === "Europe");
    expect(europe?.options.filter((choice) => choice.value === "Europe/Berlin")).toHaveLength(1);
  });
});

describe("isTimeZoneName", () => {
  it("accepts an IANA name and rejects one that isn't", () => {
    expect(isTimeZoneName("Europe/Berlin")).toBe(true);
    expect(isTimeZoneName("Mars/Olympus")).toBe(false);
  });
});
