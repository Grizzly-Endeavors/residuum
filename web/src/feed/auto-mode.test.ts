import { describe, expect, it } from "vitest";
import type { AutoModeVerdict } from "../lib/generated/AutoModeVerdict";
import { autoModeNote, blockedByAutoMode } from "./auto-mode";

const verdict = (over: Partial<AutoModeVerdict>): AutoModeVerdict => ({
  decision: "allowed",
  rule: null,
  probability: null,
  reason: null,
  input_tokens: 40,
  ...over,
});

describe("Auto Mode words", () => {
  it("names the rule and how sure the model was for a blocked call", () => {
    const blocked = verdict({ decision: "blocked", rule: "Pushing to main", probability: 0.934 });
    expect(blockedByAutoMode(blocked)).toBe(true);
    expect(autoModeNote(blocked)).toBe(
      "Blocked by Auto Mode: it matches the rule “Pushing to main” (93% sure). It didn't run.",
    );
  });

  it("says a checked call matched nothing, or which exception let it through", () => {
    expect(autoModeNote(verdict({}))).toBe("Checked by Auto Mode: no rule matched.");
    expect(
      autoModeNote(verdict({ rule: "Deleting files under tmp/", probability: 0.81 })),
    ).toContain("exception “Deleting files under tmp/” (81% sure)");
  });

  it("passes on why a call couldn't be checked", () => {
    const unchecked = verdict({ decision: "unchecked", reason: "Couldn't reach Ollama." });
    expect(blockedByAutoMode(unchecked)).toBe(false);
    expect(autoModeNote(unchecked)).toBe("Not checked by Auto Mode: Couldn't reach Ollama.");
  });

  it("treats a call Auto Mode never saw as not blocked", () => {
    expect(blockedByAutoMode(undefined)).toBe(false);
  });
});
