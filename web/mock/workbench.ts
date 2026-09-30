import type { ArtifactSummary, WorkbenchInfo } from "../src/lib/types";
import { json, text } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import { listDirectory, removePath } from "./workspace-tree";

/** The team's workbench folder, in the workspace tree the hub's state holds. */
const WORKBENCH_DIR = "team/workbench";

const MAX_ARTIFACT_NAME_LENGTH = 64;

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

/** Whether `name` is an artifact name: lowercase letters and digits in hyphen-separated words, at most 64 characters. */
function isValidArtifactName(name: string): boolean {
  return name.length <= MAX_ARTIFACT_NAME_LENGTH && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name);
}

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
function listArtifacts({ res, state }: RouteContext): void {
  const artifacts: ArtifactSummary[] = [...state.workbenchArtifacts].map(([name, artifact]) => ({
    name,
    title: pageTitle(artifact.html) ?? name,
    modified_at: artifact.modifiedAt,
    size: Buffer.byteLength(artifact.html),
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

/** Remove an artifact, and the `<name>.*` data files next to it in the workbench folder, such as its saved state. */
function deleteArtifact(ctx: RouteContext): void {
  const { res, state, hub } = ctx;
  const name = decodedParam(ctx, 0);
  if (!isValidArtifactName(name)) {
    text(
      res,
      400,
      `invalid artifact name ${JSON.stringify(name)}: use lowercase letters, digits, and single hyphens`,
    );
    return;
  }
  if (!state.workbenchArtifacts.delete(name)) {
    text(res, 404, "That artifact no longer exists. It may already have been deleted.");
    return;
  }
  const removed = new Set([`${name}.html`]);
  for (const entry of listDirectory(hub.hubState, WORKBENCH_DIR) ?? []) {
    if (entry.entry_type === "file" && entry.name.startsWith(`${name}.`)) {
      removed.add(entry.name);
      removePath(hub.hubState, `${WORKBENCH_DIR}/${entry.name}`);
    }
  }
  json(res, 200, {
    removed: [...removed].sort(),
    // The mock keeps no checkpoints, so there is nothing for Undo to restore.
    checkpoint_id: null,
  } satisfies DeleteArtifactResponse);
}

/** The workbench routes, in the unscoped `/api/...` spelling. */
export const workbenchRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/workbench/artifacts", handler: listArtifacts },
  { method: "GET", pattern: "/api/workbench/info", handler: workbenchInfo },
  { method: "DELETE", pattern: /^\/api\/workbench\/artifacts\/([^/]+)$/, handler: deleteArtifact },
];
