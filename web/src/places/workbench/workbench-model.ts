import type { AgentOverview, LiveSession } from "../../lib/hub-types";

export interface ArtifactRun {
  agent: string;
  run: LiveSession;
}

export function artifactRuns(
  overviews: Readonly<Record<string, AgentOverview>>,
  artifact: string,
): ArtifactRun[] {
  const label = `artifact:${artifact}`;
  return Object.values(overviews)
    .flatMap((overview) =>
      overview.live_sessions
        .filter((run) => run.source_label === label)
        .map((run) => ({ agent: overview.name, run })),
    )
    .sort(
      (a, b) =>
        Date.parse(a.run.started_at) - Date.parse(b.run.started_at) ||
        a.agent.localeCompare(b.agent),
    );
}

export function runningWords(count: number): string {
  return `${String(count)} ${count === 1 ? "session" : "sessions"} running`;
}
