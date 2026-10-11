import { isValidArtifactName } from "./artifact-name";
import type { MockState } from "./state";
import { byteLength, entryOf, fileVersion } from "./workspace-tree";

/** The team's workbench folder, in the workspace tree the hub's state holds. */
export const WORKBENCH_DIR = "team/workbench";

/** An artifact found in the workbench folder, as the backend's `discover_artifacts` finds it. */
export interface DiscoveredArtifact {
  name: string;
  /** `<name>.html`, or `<name>/` with an `index.html`. */
  kind: "page" | "folder";
  /** The tree path of the page the artifact opens with. */
  entryPage: string;
  /** Newest modification time among the artifact's files, in milliseconds. */
  modified: number;
  /** Total size of the artifact's files. */
  size: number;
  /** Changes whenever the artifact's files or their content do, which is what a reload hangs on. */
  stamp: string;
}

function describe(
  state: MockState,
  name: string,
  kind: DiscoveredArtifact["kind"],
  files: string[],
  entryPage: string,
): DiscoveredArtifact {
  const contents = state.workspaceFileContents;
  return {
    name,
    kind,
    entryPage,
    modified: Math.max(0, ...files.map((path) => entryOf(state, path)?.modified ?? 0)),
    size: files.reduce((sum, path) => sum + byteLength(contents[path] ?? ""), 0),
    stamp: files
      .toSorted()
      .map((path) => `${path}=${fileVersion(contents[path] ?? "")}`)
      .join(";"),
  };
}

/**
 * Every artifact in the team workbench folder, by name: a page, or a folder
 * with an `index.html`. Only those count, so saved data beside an artifact
 * (`<name>.state.json`) is not part of one. When a page and a folder share a
 * name, the folder wins.
 */
export function discoverArtifacts(state: MockState): Map<string, DiscoveredArtifact> {
  const contents = state.workspaceFileContents;
  const found = new Map<string, DiscoveredArtifact>();
  for (const { name: entry, entry_type: type } of state.workspaceFiles[WORKBENCH_DIR] ?? []) {
    const path = `${WORKBENCH_DIR}/${entry}`;
    const page = type === "file" && entry.endsWith(".html") ? entry.slice(0, -".html".length) : "";
    if (isValidArtifactName(page) && !found.has(page)) {
      found.set(page, describe(state, page, "page", [path], path));
    } else if (
      type === "directory" &&
      isValidArtifactName(entry) &&
      `${path}/index.html` in contents
    ) {
      const files = Object.keys(contents).filter((file) => file.startsWith(`${path}/`));
      found.set(entry, describe(state, entry, "folder", files, `${path}/index.html`));
    }
  }
  return found;
}

/**
 * The file `rest` names in an artifact, as `{ path, content }`, or `null` when
 * it names none. `""` and a trailing `/` mean an `index.html`. A page has no
 * files besides itself, and `.`, `..` and backslash segments name nothing.
 */
export function readArtifactFile(
  state: MockState,
  artifact: DiscoveredArtifact,
  rest: string,
): { path: string; content: string } | null {
  const segments = rest.split("/").filter((segment) => segment !== "");
  if (segments.some((segment) => segment === "." || segment === ".." || segment.includes("\\"))) {
    return null;
  }
  let path = artifact.entryPage;
  if (artifact.kind === "page") {
    if (rest !== "" && rest !== "index.html") return null;
  } else {
    const indexed = rest === "" || rest.endsWith("/") ? ["index.html"] : [];
    path = [`${WORKBENCH_DIR}/${artifact.name}`, ...segments, ...indexed].join("/");
  }
  const content = state.workspaceFileContents[path];
  return content === undefined ? null : { path, content };
}
