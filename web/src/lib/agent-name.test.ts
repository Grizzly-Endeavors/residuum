import { describe, expect, it } from "vitest";
import { DEFAULT_AGENT_NAME, agentNameProblem } from "./agent-name";

describe("agentNameProblem", () => {
  it("accepts the default and typical names", () => {
    expect(agentNameProblem(DEFAULT_AGENT_NAME)).toBeNull();
    expect(agentNameProblem("work-bot")).toBeNull();
    expect(agentNameProblem("a1")).toBeNull();
    expect(agentNameProblem("x".repeat(24))).toBeNull();
  });

  it("rejects empty and over-long names", () => {
    expect(agentNameProblem("")).not.toBeNull();
    expect(agentNameProblem("x".repeat(25))).not.toBeNull();
  });

  it("rejects characters outside lowercase letters, digits, and hyphens", () => {
    for (const bad of ["Upper", "has space", "under_score", "dot.name", "émile"]) {
      expect(agentNameProblem(bad), bad).not.toBeNull();
    }
  });

  it("rejects leading and trailing hyphens", () => {
    expect(agentNameProblem("-lead")).not.toBeNull();
    expect(agentNameProblem("trail-")).not.toBeNull();
  });

  it("rejects the reserved names", () => {
    for (const reserved of ["hub", "team", "agents"]) {
      expect(agentNameProblem(reserved), reserved).toMatch(/reserved/);
    }
  });
});
