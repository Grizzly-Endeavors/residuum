// Which of the build's files the service worker precaches, and the version
// that names them. The worker holds the whole app shell offline, so the list is
// the document, everything under `assets/` (scripts, styles and fonts, lazy
// chunks included) and the icons. Anything else the build ships (the manifest,
// the MCP catalog, the font licenses) is fetched from the hub when it is used.

import { createHash } from "node:crypto";

/** A file of the build: its path under the output directory, with forward slashes, and its content. */
export interface BuildFile {
  path: string;
  bytes: Uint8Array;
}

const SHELL_FILES: ReadonlySet<string> = new Set(["index.html", "favicon.svg"]);
const SHELL_DIRECTORIES = ["assets/", "icons/"] as const;

/** Whether the worker precaches the build file at `path`. */
export function isPrecached(path: string): boolean {
  return SHELL_FILES.has(path) || SHELL_DIRECTORIES.some((directory) => path.startsWith(directory));
}

export interface PrecachePlan {
  /** Each precached file as the URL the app requests it by, sorted. */
  urls: string[];
  /**
   * A hash over every precached file's path and content. A build whose shell
   * differs in any byte, `index.html` and the icons included, gets another
   * version, so its worker differs and a browser installs it.
   */
  version: string;
}

export function planPrecache(files: readonly BuildFile[]): PrecachePlan {
  const shell = files
    .filter((file) => isPrecached(file.path))
    .sort((a, b) => Number(a.path > b.path) - Number(a.path < b.path));
  const hash = createHash("sha256");
  for (const file of shell) {
    hash.update(`${file.path}\0`);
    hash.update(createHash("sha256").update(file.bytes).digest());
  }
  return {
    urls: shell.map((file) => `/${file.path}`),
    version: hash.digest("hex").slice(0, 12),
  };
}
