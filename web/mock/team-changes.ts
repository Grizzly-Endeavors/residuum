import type { ServerMessage, WorkspaceChange } from "../src/lib/generated/protocol";
import type { MockHub } from "./state";
import { normalizeWatchPrefix, watchPrefixProblem } from "./sockets";
import { discoverArtifacts } from "./workbench-files";
import { pathKind, removePath, writeFile } from "./workspace-tree";

/** What a change to a team file sent, for the caller to check. */
export interface TeamChangeOutcome {
  /** The `workspace_changed` batch: one change, in the change feed's `team/...` paths. */
  changes: WorkspaceChange[];
  /** The artifacts whose `artifact_updated` and `artifact_removed` frames followed. */
  artifacts: { updated: string[]; removed: string[] };
}

/** A change the mock refused, and the status it answers with. */
export interface TeamChangeRefusal {
  status: number;
  error: string;
}

/**
 * Change a team file the way an agent does, then send what the real system
 * sends once its change feed sees it: `workspace_changed` to the pages that
 * watch the path, on the hub socket and on every agent's, and for a change to
 * an artifact's page or folder `artifact_updated` or `artifact_removed` on
 * the hub socket and on every agent's socket. `content: null` removes the file, or the folder and
 * everything in it. As in the feed, a new folder is reported alone (it stands
 * for what it holds), and a rewrite that leaves an artifact's files as they
 * were sends no artifact frame.
 *
 * `path` is in the file API's namespace, so `team/workbench/tip-splitter.html`.
 */
export function changeTeamFile(
  hub: MockHub,
  requested: string,
  content: string | null,
): TeamChangeOutcome | TeamChangeRefusal {
  const team = hub.hubState;
  const path = normalizeWatchPrefix(requested);
  const segments = path.split("/");
  if (segments[0] !== "team" || segments.length < 2 || watchPrefixProblem(requested) !== null) {
    return {
      status: 422,
      error: `mock: \`path\` must be under team/, not ${JSON.stringify(requested)}`,
    };
  }
  const kind = pathKind(team, path);
  if (content === null && kind === null) {
    return { status: 404, error: `mock: there is nothing at ${path} to remove` };
  }
  if (content !== null && kind === "directory") {
    return { status: 422, error: `mock: ${path} is a folder, so it has no content to write` };
  }

  const artifactsBefore = discoverArtifacts(team);
  let change: WorkspaceChange = { path, kind: "modified" };
  if (content === null) {
    change = { path, kind: "removed" };
    removePath(team, path);
  } else {
    const newest = segments
      .map((_, at) => segments.slice(0, at + 1).join("/"))
      .find((prefix) => pathKind(team, prefix) === null);
    if (newest !== undefined) change = { path: newest, kind: "created" };
    writeFile(team, path, content);
  }

  const artifactsAfter = discoverArtifacts(team);
  const updated = [...artifactsAfter.values()]
    .filter(({ name, stamp }) => artifactsBefore.get(name)?.stamp !== stamp)
    .map(({ name }) => name)
    .sort();
  const removed = [...artifactsBefore.keys()].filter((name) => !artifactsAfter.has(name)).sort();

  const changes = [change];
  hub.broadcast({ type: "workspace_changed", changes });
  // The hub watches the workbench itself, so its artifact frames don't depend on an agent running.
  for (const name of updated) hub.broadcast({ type: "artifact_updated", name });
  for (const name of removed) hub.broadcast({ type: "artifact_removed", name });
  const frames: ServerMessage[] = [
    { type: "workspace_changed", changes },
    ...updated.map((name): ServerMessage => ({ type: "artifact_updated", name })),
    ...removed.map((name): ServerMessage => ({ type: "artifact_removed", name })),
  ];
  for (const agent of hub.agents.values()) {
    for (const frame of frames) agent.state.broadcast(frame);
  }
  return { changes, artifacts: { updated, removed } };
}
