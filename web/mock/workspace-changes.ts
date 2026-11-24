import type { ServerMessage, WorkspaceChange } from "../src/lib/generated/protocol";
import type { MockAgent, MockHub, MockState } from "./state";
import { normalizeWatchPrefix, watchPrefixProblem } from "./sockets";
import { discoverArtifacts } from "./workbench-files";
import { pathKind, removePath, writeFile } from "./workspace-tree";

export interface TeamChangeOutcome {
  changes: WorkspaceChange[];
  artifacts: { updated: string[]; removed: string[] };
}

export interface AgentChangeOutcome {
  changes: WorkspaceChange[];
}

export interface FileChangeRefusal {
  status: number;
  error: string;
}

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
