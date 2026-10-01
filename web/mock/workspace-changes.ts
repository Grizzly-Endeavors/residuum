import type { ServerMessage, WorkspaceChange } from "../src/lib/generated/protocol";
import type { MockAgent, MockHub, MockState } from "./state";
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

/** What a change to a file in an agent's own workspace sent. */
export interface AgentChangeOutcome {
  /** The `workspace_changed` batch: one change, in the agent's workspace paths. */
  changes: WorkspaceChange[];
}

/** A change the mock refused, and the status it answers with. */
export interface FileChangeRefusal {
  status: number;
  error: string;
}

/**
 * Write `content` at `path` in `tree`, or remove what is there for `null`,
 * and say what the change feed reports for it. As in the feed, a new folder
 * is reported alone, standing for what it holds.
 */
function applyFileChange(
  tree: MockState,
  path: string,
  content: string | null,
): WorkspaceChange | FileChangeRefusal {
  const kind = pathKind(tree, path);
  if (content === null && kind === null) {
    return { status: 404, error: `mock: there is nothing at ${path} to remove` };
  }
  if (content !== null && kind === "directory") {
    return { status: 422, error: `mock: ${path} is a folder, so it has no content to write` };
  }
  if (content === null) {
    removePath(tree, path);
    return { path, kind: "removed" };
  }
  const segments = path.split("/");
  const newest = segments
    .map((_, at) => segments.slice(0, at + 1).join("/"))
    .find((prefix) => pathKind(tree, prefix) === null);
  writeFile(tree, path, content);
  return newest === undefined ? { path, kind: "modified" } : { path: newest, kind: "created" };
}

/**
 * Change a team file the way an agent does, then send what the real system
 * sends once its change feed sees it: `workspace_changed` to the pages that
 * watch the path, on the hub socket and on every agent's, and for a change to
 * an artifact's page or folder `artifact_updated` or `artifact_removed` on
 * the hub socket and on every agent's socket. `content: null` removes the file, or the folder and
 * everything in it. A rewrite that leaves an artifact's files as they were
 * sends no artifact frame.
 *
 * `path` is in the file API's namespace, so `team/workbench/tip-splitter.html`.
 */
export function changeTeamFile(
  hub: MockHub,
  requested: string,
  content: string | null,
): TeamChangeOutcome | FileChangeRefusal {
  const team = hub.hubState;
  const path = normalizeWatchPrefix(requested);
  const segments = path.split("/");
  if (segments[0] !== "team" || segments.length < 2 || watchPrefixProblem(requested) !== null) {
    return {
      status: 422,
      error: `mock: \`path\` must be under team/, not ${JSON.stringify(requested)}`,
    };
  }

  const artifactsBefore = discoverArtifacts(team);
  const change = applyFileChange(team, path, content);
  if ("status" in change) return change;
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

/**
 * Change a file in `agent`'s own workspace the way the agent does, then send
 * `workspace_changed` to the pages on its socket that watch the path.
 * `content: null` removes the file, or the folder and everything in it.
 *
 * `path` is relative to the agent's workspace, so `notes/today.md`; team files
 * change through `changeTeamFile`.
 */
export function changeAgentFile(
  agent: MockAgent,
  requested: string,
  content: string | null,
): AgentChangeOutcome | FileChangeRefusal {
  const path = normalizeWatchPrefix(requested);
  if (path === "" || path.split("/")[0] === "team" || watchPrefixProblem(requested) !== null) {
    return {
      status: 422,
      error: `mock: \`path\` must be a file in ${agent.name}'s own workspace, not ${JSON.stringify(requested)} (team files change through /api/mock/team-file)`,
    };
  }
  const change = applyFileChange(agent.state, path, content);
  if ("status" in change) return change;
  const changes = [change];
  agent.state.broadcast({ type: "workspace_changed", changes });
  return { changes };
}
