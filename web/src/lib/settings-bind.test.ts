import { describe, expect, it } from "vitest";
import { numberOfText, textOfNumber } from "./settings-bind";

describe("numberOfText", () => {
  it("reads the number a field's text holds", () => {
    expect(numberOfText("7700")).toBe(7700);
    expect(numberOfText(" 3 ")).toBe(3);
    expect(numberOfText("0.5")).toBe(0.5);
  });

  it("reads a number a number input left in the field", () => {
    expect(numberOfText(12)).toBe(12);
  });

  it("reads an empty or non-numeric field as no number", () => {
    expect(numberOfText("")).toBeNull();
    expect(numberOfText("   ")).toBeNull();
    expect(numberOfText("abc")).toBeNull();
    expect(numberOfText(Number.NaN)).toBeNull();
  });
});

describe("textOfNumber", () => {
  it("keeps the box's number as text and an empty box as empty text", () => {
    expect(textOfNumber(8)).toBe("8");
    expect(textOfNumber(0)).toBe("0");
    expect(textOfNumber(null)).toBe("");
  });
});
