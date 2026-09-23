import { describe, expect, it } from "vitest";
import {
  SESSION_CATEGORIES,
  categoryHeading,
  groupByCategory,
  sessionArtifact,
  sessionSourceText,
} from "./session-format";
import type { SessionCategory, SessionSummary } from "./types";

function session(runId: string, category: SessionCategory, sourceLabel: string): SessionSummary {
  return {
    address: `${category}-x-0001`,
    run_id: runId,
    category,
    source_label: sourceLabel,
    state: "running",
    spawner: null,
    depth: 1,
    purpose: "",
    started_at: "2026-09-23T12:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
  };
}

describe("artifact sessions", () => {
  it("group under their own category, apart from spawned work", () => {
    const groups = groupByCategory([
      session("a1", "artifact", "artifact:wiki-graph"),
      session("s1", "spawned", "agent:researcher"),
      session("a2", "artifact", "artifact:chart"),
    ]);
    expect(groups.artifact.map((s) => s.run_id)).toEqual(["a1", "a2"]);
    expect(groups.spawned.map((s) => s.run_id)).toEqual(["s1"]);
    expect(groups.scheduled).toEqual([]);
    expect(SESSION_CATEGORIES).toContain("artifact");
    expect(categoryHeading("artifact")).toBe("Artifacts");
  });

  it("are labelled with the artifact that started them", () => {
    const started = session("a1", "artifact", "artifact:wiki-graph");
    expect(sessionArtifact(started)).toBe("wiki-graph");
    expect(sessionSourceText(started)).toBe("wiki-graph");

    const spawned = session("s1", "spawned", "agent:researcher");
    expect(sessionArtifact(spawned)).toBeNull();
    expect(sessionSourceText(spawned)).toBe("agent:researcher");
  });
});
