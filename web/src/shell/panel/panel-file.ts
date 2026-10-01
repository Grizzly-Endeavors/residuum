// Which tree a `panel=file:<path>` names (design §3): on an agent's places, a
// path in that agent's workspace (where `team/…` reaches the team's files);
// on Shared files, a path in the team's folder.

import type { WorkspaceScope } from "../../lib/hub-types";
import { isAgentPlace, type Place } from "../../lib/routes";

export interface FileSource {
  /** The agent whose workspace holds the path; null for the team's folder. */
  agent: string | null;
  scope: WorkspaceScope;
}

/** Where a file panel's path is read on `place`, or null on a place that can't show one. */
export function fileSourceFor(place: Place): FileSource | null {
  if (isAgentPlace(place)) return { agent: place.agent, scope: "agent" };
  if (place.kind === "shared-files") return { agent: null, scope: "team" };
  return null;
}

/** The last segment of a path, for a title. */
export function fileName(path: string): string {
  const segments = path.split("/").filter((segment) => segment !== "");
  return segments.at(-1) ?? path;
}
