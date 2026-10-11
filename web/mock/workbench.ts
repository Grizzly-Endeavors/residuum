import type { ArtifactSummary, WorkbenchInfo } from "../src/lib/types";
import { isValidArtifactName } from "./artifact-name";
import { checkpointBeforeAction } from "./checkpoints";
import { json, text } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import { discoverArtifacts, WORKBENCH_DIR } from "./workbench-files";
import { listDirectory, pathKind, removePath } from "./workspace-tree";

/** The character references the backend decodes in a page's title. */
const TITLE_ENTITIES: ReadonlyArray<readonly [string, string]> = [
  ["&lt;", "<"],
  ["&gt;", ">"],
  ["&quot;", '"'],
  ["&#39;", "'"],
  ["&apos;", "'"],
  ["&nbsp;", " "],
  ["&amp;", "&"],
];

/** The text of the page's first `<title>`, with its whitespace collapsed, or `null` when it has none. */
function pageTitle(html: string): string | null {
  const raw = /<title(?:\s[^>]*)?>([\s\S]*?)<\/title/i.exec(html)?.[1];
  if (raw === undefined) return null;
  const collapsed = raw.split(/\s+/).filter(Boolean).join(" ");
  const decoded = TITLE_ENTITIES.reduce(
    (title, [entity, char]) => title.replaceAll(entity, char),
    collapsed,
  );
  return decoded === "" ? null : decoded;
}

/** Every artifact, most recently modified first. */
function listArtifacts({ res, hub }: RouteContext): void {
  const team = hub.hubState;
  const artifacts: ArtifactSummary[] = [...discoverArtifacts(team).values()].map((artifact) => ({
    name: artifact.name,
    title: pageTitle(team.workspaceFileContents[artifact.entryPage] ?? "") ?? artifact.name,
    modified_at: new Date(artifact.modified).toISOString(),
    size: artifact.size,
  }));
  artifacts.sort(
    (a, b) => b.modified_at.localeCompare(a.modified_at) || a.name.localeCompare(b.name),
  );
  json(res, 200, artifacts);
}

/** Where artifacts are served: the mock artifacts listener's port, once it is up. There is no relay. */
function workbenchInfo({ res, state }: RouteContext): void {
  json(res, 200, {
    port: state.workbenchPort,
    unavailable_reason:
      state.workbenchPort === null ? "The mock artifacts listener isn't up yet." : null,
    relay: null,
  } satisfies WorkbenchInfo);
}

/** What `DELETE` answers: what it removed, and the checkpoint that can bring it back. */
interface DeleteArtifactResponse {
  removed: string[];
  checkpoint_id: string | null;
}

/**
 * Remove an artifact (its page, or its folder), and the `<name>.*` data files
 * next to it in the workbench folder, such as its saved state. The team is
 * checkpointed first, so restoring the paths it lists undoes the delete.
 */
function deleteArtifact(ctx: RouteContext): void {
  const { res, hub } = ctx;
  const team = hub.hubState;
  const name = decodedParam(ctx, 0);
  if (!isValidArtifactName(name)) {
    text(
      res,
      400,
      `invalid artifact name ${JSON.stringify(name)}: use lowercase letters, digits, and single hyphens`,
    );
    return;
  }
  const folder = `${WORKBENCH_DIR}/${name}`;
  const hasFolder = pathKind(team, folder) === "directory";
  if (!hasFolder && pathKind(team, `${folder}.html`) !== "file") {
    text(res, 404, "That artifact no longer exists. It may already have been deleted.");
    return;
  }
  const checkpointId = checkpointBeforeAction(team, "team", `delete workbench artifact ${name}`);
  const removed: string[] = [];
  if (hasFolder) {
    removePath(team, folder);
    removed.push(`${name}/`);
  }
  for (const entry of listDirectory(team, WORKBENCH_DIR) ?? []) {
    if (entry.entry_type === "file" && entry.name.startsWith(`${name}.`)) {
      removePath(team, `${WORKBENCH_DIR}/${entry.name}`);
      removed.push(entry.name);
    }
  }
  json(res, 200, {
    removed: removed.sort(),
    checkpoint_id: checkpointId,
  } satisfies DeleteArtifactResponse);
}

/** The workbench routes, in the unscoped `/api/...` spelling. */
export const workbenchRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/workbench/artifacts", handler: listArtifacts },
  { method: "GET", pattern: "/api/workbench/info", handler: workbenchInfo },
  { method: "DELETE", pattern: /^\/api\/workbench\/artifacts\/([^/]+)$/, handler: deleteArtifact },
];
