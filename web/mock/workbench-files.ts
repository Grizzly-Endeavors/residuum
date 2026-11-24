import { isValidArtifactName } from "./artifact-name";
import type { MockState } from "./state";
import { byteLength, entryOf, fileVersion } from "./workspace-tree";

export const WORKBENCH_DIR = "team/workbench";

export interface DiscoveredArtifact {
  name: string;
  kind: "page" | "folder";
  entryPage: string;
  modified: number;
  size: number;
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
