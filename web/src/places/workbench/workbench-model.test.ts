import { describe, expect, it } from "vitest";
import type { AgentOverview, LiveSession } from "../../lib/hub-types";
import { artifactRuns, runningWords } from "./workbench-model";

function liveRun(runId: string, sourceLabel: string, startedAt: string): LiveSession {
  return {
    address: `artifact-x-${runId}`,
    run_id: runId,
    category: "artifact",
    source_label: sourceLabel,
    purpose: `run ${runId}`,
    state: "running",
    started_at: startedAt,
  };
}

function overview(name: string, live: LiveSession[]): AgentOverview {
  return {
    name,
    last_message: null,
    live_sessions: live,
    upcoming: [],
    inbox_unread: 0,
    outbound_problems: [],
  };
}

describe("artifactRuns", () => {
  const overviews = {
    atlas: overview("atlas", [
      liveRun("a1", "artifact:wiki-graph", "2026-03-14T11:58:00Z"),
      liveRun("a2", "artifact:chart", "2026-03-14T11:50:00Z"),
      liveRun("a3", "agent:researcher", "2026-03-14T11:40:00Z"),
    ]),
    scout: overview("scout", [liveRun("s1", "artifact:wiki-graph", "2026-03-14T11:55:00Z")]),
    drifter: overview("drifter", []),
  };

  it("finds an artifact's runs on every agent by their source label, oldest first", () => {
    expect(
      artifactRuns(overviews, "wiki-graph").map(({ agent, run }) => [agent, run.run_id]),
    ).toEqual([
      ["scout", "s1"],
      ["atlas", "a1"],
    ]);
  });

  it("leaves out other artifacts' runs and runs no artifact started", () => {
    expect(artifactRuns(overviews, "chart").map(({ run }) => run.run_id)).toEqual(["a2"]);
    expect(artifactRuns(overviews, "wiki")).toEqual([]);
    expect(artifactRuns({}, "wiki-graph")).toEqual([]);
  });
});

describe("runningWords", () => {
  it("counts one session and several", () => {
    expect(runningWords(1)).toBe("1 session running");
    expect(runningWords(3)).toBe("3 sessions running");
  });
});
