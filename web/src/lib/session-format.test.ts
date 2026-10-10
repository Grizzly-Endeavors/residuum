import { describe, expect, it } from "vitest";
import {
  finishedOutcome,
  isStoppableState,
  outboundStatus,
  runKind,
  runStatus,
  sessionArtifact,
} from "./session-format";
import type {
  OutboundA2aTaskSummary,
  SessionCategory,
  SessionState,
  SessionSummary,
} from "./types";

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
    usage: { input_tokens: 0, output_tokens: 0, context_tokens: null, tool_calls: 0 },
    outcome: null,
    error: null,
    error_details: null,
    overlap: null,
  };
}

describe("a run's words", () => {
  const NOW = Date.parse("2026-09-23T12:04:00Z");

  it("names its kind in plain words, and who started a spawned one", () => {
    expect(runKind("atlas", session("e", "external", "discord:#builds"))).toBe("From another app");
    expect(runKind("atlas", session("p", "scheduled", "pulse:inbox"))).toBe("Scheduled");
    expect(runKind("atlas", session("a", "artifact", "artifact:chart"))).toBe(
      "From a workbench page",
    );
    const spawned = session("s", "spawned", "agent:researcher");
    expect(runKind("atlas", { ...spawned, spawner: "main" })).toBe("Started by atlas");
    expect(runKind("atlas", { ...spawned, spawner: "spawned-a-1" })).toBe("Started by spawned-a-1");
    expect(runKind("atlas", { ...spawned, spawner: null })).toBe("Started by you");
    expect(runKind("atlas", { ...spawned, category: "artifact", spawner: null })).toBe(
      "From a workbench page",
    );
  });

  it("says how a live run is doing, with how long it has run", () => {
    const run = session("r", "spawned", "agent:researcher");
    expect(runStatus(run, NOW)).toEqual({ tone: "working", text: "Working, 4m" });
    expect(runStatus({ ...run, state: "idle" }, NOW)).toEqual({ tone: "quiet", text: "Idle, 4m" });
    expect(runStatus({ ...run, state: "forking" }, NOW).text).toBe("Starting");
  });

  it("says how a finished run ended", () => {
    const done: SessionSummary = {
      ...session("r", "spawned", "agent:researcher", "completed"),
      completed_at: "2026-09-23T12:12:00Z",
    };
    expect(runStatus(done, NOW)).toEqual({ tone: "done", text: "Finished" });
    expect(finishedOutcome(done)).toBe("Finished after 12m");
    const failed = { ...done, outcome: "failed" as const, error: "the site timed out" };
    expect(runStatus(failed, NOW)).toEqual({ tone: "failed", text: "Failed" });
    expect(finishedOutcome(failed)).toBe("Failed: the site timed out");
    expect(finishedOutcome({ ...done, outcome: "cancelled" })).toBe("Stopped after 12m");
    expect(finishedOutcome({ ...done, interrupted: true })).toBe("Cut short when Residuum stopped");
  });

  it("says where a task sent to another agent stands", () => {
    const task: OutboundA2aTaskSummary = {
      task_id: "t1",
      agent: "laptop",
      sender_address: "main",
      state: "working",
      status_text: null,
      open: true,
      started_at: "2026-09-23T11:00:00Z",
      unreachable_since: null,
    };
    expect(outboundStatus(task, NOW)).toEqual({ tone: "working", text: "Working" });
    expect(outboundStatus({ ...task, state: "auth_required" }, NOW).text).toBe(
      "Waiting on sign-in",
    );
    expect(outboundStatus({ ...task, unreachable_since: "2026-09-23T11:47:00Z" }, NOW)).toEqual({
      tone: "failed",
      text: "Can't reach laptop for 17m",
    });
  });
});

describe("artifact sessions", () => {
  it("are labelled with the artifact that started them", () => {
    const started = session("a1", "artifact", "artifact:wiki-graph");
    expect(sessionArtifact(started)).toBe("wiki-graph");

    const spawned = session("s1", "spawned", "agent:researcher");
    expect(sessionArtifact(spawned)).toBeNull();
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
