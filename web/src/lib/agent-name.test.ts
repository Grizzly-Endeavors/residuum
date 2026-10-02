import { describe, expect, it } from "vitest";
import {
  DEFAULT_AGENT_NAME,
  agentNameProblem,
  allocateSlug,
  isAgentSlug,
  newAgentNameProblem,
  slugBase,
} from "./agent-name";

describe("agentNameProblem", () => {
  it("accepts capitals, spaces, and letters from other languages", () => {
    expect(agentNameProblem(DEFAULT_AGENT_NAME)).toBeNull();
    expect(agentNameProblem("Scout")).toBeNull();
    expect(agentNameProblem("Research Desk")).toBeNull();
    expect(agentNameProblem("José")).toBeNull();
    expect(agentNameProblem("研究")).toBeNull();
    expect(agentNameProblem("x".repeat(32))).toBeNull();
  });

  it("rejects empty and over-long names", () => {
    expect(agentNameProblem("")).not.toBeNull();
    expect(agentNameProblem("x".repeat(33))).toMatch(/32/);
  });

  it("rejects characters outside letters, numbers, spaces, hyphens, and apostrophes", () => {
    for (const bad of ["under_score", "Scout!", "nope."]) {
      expect(agentNameProblem(bad), bad).not.toBeNull();
    }
  });

  it("rejects a leading or trailing hyphen or apostrophe", () => {
    expect(agentNameProblem("-lead")).toMatch(/hyphen/);
    expect(agentNameProblem("trail-")).toMatch(/hyphen/);
    expect(agentNameProblem("'lead")).toMatch(/apostrophe/);
  });

  it("rejects the reserved names, ignoring case", () => {
    for (const reserved of ["hub", "Team", "agents"]) {
      expect(agentNameProblem(reserved), reserved).toMatch(/reserved/);
    }
  });
});

describe("newAgentNameProblem", () => {
  it("accepts a name nobody has", () => {
    expect(newAgentNameProblem("Research Desk", ["atlas", "scout"])).toBeNull();
  });

  it("treats a different case as the same agent", () => {
    expect(newAgentNameProblem("Atlas", ["atlas"])).toMatch(/already have/);
    expect(newAgentNameProblem("Research Desk", ["research-desk"])).toBeNull();
  });
});

describe("isAgentSlug", () => {
  it("accepts a folder name and rejects a typed name", () => {
    expect(isAgentSlug("research-desk")).toBe(true);
    expect(isAgentSlug("Scout")).toBe(false);
    expect(isAgentSlug("Research Desk")).toBe(false);
    expect(isAgentSlug("hub")).toBe(false);
  });
});

describe("slugBase", () => {
  it("keeps letters and drops accents", () => {
    expect(slugBase("Research Desk")).toBe("research-desk");
    expect(slugBase("Atlas")).toBe("atlas");
    expect(slugBase("José")).toBe("jose");
    expect(slugBase("O'Brien")).toBe("obrien");
    expect(slugBase("Straße")).toBe("strasse");
  });

  it("gives a name with no ASCII letters a stable slug", () => {
    // Locked to the same value as `src/config/agent_name.rs`.
    expect(slugBase("研究")).toBe("n310509ec");
  });

  it("adds a suffix when the folder is taken, and stays within 24 characters", () => {
    expect(allocateSlug("research-desk", (candidate) => candidate === "research-desk")).toBe(
      "research-desk-2",
    );
    const long = "a".repeat(24);
    const suffixed = allocateSlug(long, (candidate) => candidate === long);
    expect(suffixed).toHaveLength(24);
    expect(suffixed.endsWith("-2")).toBe(true);
  });
});
