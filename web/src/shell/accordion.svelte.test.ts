import { describe, expect, it } from "vitest";
import { RailAccordion } from "./accordion.svelte";

describe("RailAccordion", () => {
  it("keeps one agent open: opening another closes the first, and a second press closes it", () => {
    const accordion = new RailAccordion();
    accordion.toggle("atlas");
    expect(accordion.open).toBe("atlas");
    accordion.toggle("scout");
    expect(accordion.open).toBe("scout");
    accordion.toggle("scout");
    expect(accordion.open).toBeNull();
  });

  it("opens the viewed agent on load", () => {
    const accordion = new RailAccordion();
    accordion.follow("atlas");
    expect(accordion.open).toBe("atlas");
  });

  it("leaves the user's choice alone while the viewed agent stays the same", () => {
    const accordion = new RailAccordion();
    accordion.follow("atlas");
    accordion.toggle("atlas");
    accordion.follow("atlas");
    expect(accordion.open).toBeNull();

    accordion.toggle("scout");
    accordion.follow("atlas");
    expect(accordion.open).toBe("scout");
  });

  it("opens an agent reached from another agent or from a place with none", () => {
    const accordion = new RailAccordion();
    accordion.follow("atlas");
    accordion.follow("scout");
    expect(accordion.open).toBe("scout");

    accordion.toggle("scout");
    accordion.follow(null);
    expect(accordion.open).toBeNull();
    accordion.follow("scout");
    expect(accordion.open).toBe("scout");
  });
});
