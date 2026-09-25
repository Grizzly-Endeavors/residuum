import { describe, expect, it } from "vitest";
import {
  SESSION_CATEGORIES,
  categoryHeading,
  groupByCategory,
  isStoppableState,
  sessionArtifact,
  sessionSourceText,
  sessionsStartedByArtifact,
} from "./session-format";
import type { SessionCategory, SessionState, SessionSummary } from "./types";

function session(
  runId: string,
  category: SessionCategory,
  sourceLabel: string,
  state: SessionState = "running",
): SessionSummary {
  return {
    address: `${category}-x-${runId}`,
    run_id: runId,
    category,
    source_label: sourceLabel,
    state,
    spawner: null,
    depth: 1,
    purpose: "",
    started_at: "2026-09-23T12:00:00Z",
    completed_at: null,
    episode_id: null,
    interrupted: false,
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null },
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

  it("are picked out of a mixed list for one artifact's activity panel, in list order", () => {
    const sessions = [
      session("a1", "artifact", "artifact:wiki-graph"),
      session("s1", "spawned", "agent:researcher"),
      session("a2", "artifact", "artifact:chart"),
      session("a3", "artifact", "artifact:wiki-graph"),
    ];
    expect(sessionsStartedByArtifact(sessions, "wiki-graph").map((s) => s.run_id)).toEqual([
      "a1",
      "a3",
    ]);
    expect(sessionsStartedByArtifact(sessions, "chart").map((s) => s.run_id)).toEqual(["a2"]);
    expect(sessionsStartedByArtifact(sessions, "no-such-artifact")).toEqual([]);
  });

  it("stopping one session leaves the artifact's other sessions in the panel's list", () => {
    const sessions = [
      session("a1", "artifact", "artifact:wiki-graph", "running"),
      session("a2", "artifact", "artifact:wiki-graph", "idle"),
    ];
    // A frame moving a1 toward completing (as a stop does) mutates only that
    // entry; a2 is untouched and still shows in the filtered list.
    const stopped: SessionSummary[] = sessions.map((s) =>
      s.run_id === "a1" ? { ...s, state: "completing" as const } : s,
    );
    const stillListed = sessionsStartedByArtifact(
      stopped.filter((s) => s.state !== "completed"),
      "wiki-graph",
    );
    expect(stillListed.map((s) => s.run_id)).toEqual(["a1", "a2"]);
    expect(stillListed.find((s) => s.run_id === "a2")?.state).toBe("idle");
  });
});

describe("isStoppableState", () => {
  it.each([
    ["forking", true],
    ["queued", true],
    ["running", true],
    ["idle", true],
    ["completing", false],
    ["completed", false],
  ] satisfies [SessionState, boolean][])("%s is stoppable: %s", (state, expected) => {
    expect(isStoppableState(state)).toBe(expected);
  });
});
