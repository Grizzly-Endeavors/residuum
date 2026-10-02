// An artifact's running sessions, on any agent, found in the overview by the
// source label every session an artifact starts carries (`artifact:<name>`),
// and the words the Workbench shows for them.

import type { AgentOverview, LiveSession } from "../../lib/hub-types";

/** A run an artifact started, and the agent it runs on. */
export interface ArtifactRun {
  agent: string;
  run: LiveSession;
}

/** An artifact's running sessions on every agent, oldest first. */
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

/** "1 session running", "3 sessions running". */
export function runningWords(count: number): string {
  return `${String(count)} ${count === 1 ? "session" : "sessions"} running`;
}
