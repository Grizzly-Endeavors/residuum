import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { extname, resolve } from "node:path";
import { isValidArtifactName } from "./artifact-name";
import { WEB_ROOT } from "./assets";
import { MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./constants";
import type { MockState } from "./state";
import { discoverArtifacts, readArtifactFile } from "./workbench-files";

/**
 * The byte offset of the first `<tag` in `lower` whose name ends at `>`, `/`
 * or whitespace, so `head` never matches `<header`, or `-1` when there is none.
 */
function findTag(lower: string, tag: string): number {
  const needle = `<${tag}`;
  for (let at = lower.indexOf(needle); at !== -1; at = lower.indexOf(needle, at + needle.length)) {
    if (/[\s>/]/.test(lower[at + needle.length] ?? "")) return at;
  }
  return -1;
}

/** `value` as JSON for a `<script>`, where no `</` can close the tag early. */
function embedAsScriptJson(value: unknown): string {
  return JSON.stringify(value).replaceAll("</", "<\\/");
}

/**
 * Serve an HTML page the way the artifacts listener does (`inject_sdk`): the
 * SDK runs before the page's own scripts, inserted right after the `<head>`
 * open tag, else after `<html>`, else after the doctype, else at the very
 * start. The artifact's name, the mock version and the mock feature list are
 * embedded for `residuum.artifact`, `residuum.version` and `residuum.features`,
 * in a block that keeps them out of the page's global scope.
 */
export function workbenchPage(html: string, artifactName: string): string {
  const sdk = readFileSync(resolve(WEB_ROOT, "..", "assets", "workbench", "sdk.js"), "utf-8");
  const context =
    `const __RESIDUUM_ARTIFACT__=${embedAsScriptJson(artifactName)};` +
    `const __RESIDUUM_VERSION__=${embedAsScriptJson(MOCK_RESIDUUM_VERSION)};` +
    `const __RESIDUUM_FEATURES__=${embedAsScriptJson(MOCK_FEATURES)};`;
  // Only ASCII letters change case, so offsets in `lower` are offsets in `html`.
  const lower = html.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
  let insertAt = 0;
  for (const tag of ["head", "html", "!doctype"]) {
    const open = findTag(lower, tag);
    const close = open === -1 ? -1 : lower.indexOf(">", open);
    if (close !== -1) {
      insertAt = close + 1;
      break;
    }
  }
  return `${html.slice(0, insertAt)}<script>{${context}${sdk}}</script>${html.slice(insertAt)}`;
}

/** What a file's extension says it is; text types are served as UTF-8. */
const CONTENT_TYPES: Readonly<Record<string, string>> = {
  ".html": "text/html",
  ".htm": "text/html",
  ".css": "text/css",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".txt": "text/plain",
  ".md": "text/markdown",
  ".csv": "text/csv",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
  ".wasm": "application/wasm",
};

/** A minimal HTML page, since these responses land in the artifact's frame. */
function page(res: ServerResponse, status: number, message: string): void {
  const escaped = message.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
  res.writeHead(status, { "Content-Type": "text/html; charset=utf-8" });
  res.end(
    `<!doctype html><meta charset=utf-8><title>Workbench</title><body style="font:14px sans-serif;color:#a8a29e;background:#12100e;padding:2rem"><p>${escaped}</p></body>`,
  );
}

/** `decodeURIComponent`, or `null` for a malformed escape. */
function decode(part: string): string | null {
  try {
    return decodeURIComponent(part);
  } catch {
    return null;
  }
}

/**
 * Answer one request like the real listener's read-only routes: `/` says
 * where artifacts open, `/{artifact}` redirects to `/{artifact}/` keeping the
 * query, `/{artifact}/` is the artifact's page and `/{artifact}/{path}` a
 * file in a folder artifact.
 */
function serve(state: MockState, req: IncomingMessage, res: ServerResponse): void {
  if (req.method !== "GET" && req.method !== "HEAD") {
    res.writeHead(405, { Allow: "GET,HEAD" });
    res.end();
    return;
  }
  const target = req.url ?? "/";
  const queryAt = target.indexOf("?");
  const query = queryAt === -1 ? "" : target.slice(queryAt);
  const match = /^\/([^/]*)(?:\/(.*))?$/s.exec(queryAt === -1 ? target : target.slice(0, queryAt));
  const name = decode(match?.[1] ?? "");
  const rest = match?.[2] === undefined ? undefined : decode(match[2]);
  if (match === null || name === null || rest === null) {
    page(res, 404, "Nothing here.");
    return;
  }
  if (name === "" && rest === undefined) {
    page(res, 200, "Workbench artifacts open from the Workbench page in Residuum.");
    return;
  }
  const artifact = isValidArtifactName(name) ? discoverArtifacts(state).get(name) : undefined;
  if (artifact === undefined) {
    page(res, 404, `There's no workbench artifact named "${name}".`);
    return;
  }
  if (rest === undefined) {
    res.writeHead(308, { Location: `/${name}/${query}` });
    res.end();
    return;
  }
  const file = readArtifactFile(state, artifact, rest);
  if (file === null) {
    page(res, 404, `The artifact "${name}" has no file "${rest}".`);
    return;
  }
  const type = CONTENT_TYPES[extname(file.path).toLowerCase()] ?? "application/octet-stream";
  res.writeHead(200, {
    "Content-Type": type.startsWith("text/") ? `${type}; charset=utf-8` : type,
    // Artifacts reload live while the agent edits them.
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  });
  res.end(type === "text/html" ? workbenchPage(file.content, name) : file.content);
}

/**
 * A second origin for artifacts, like the gateway's artifacts listener. It
 * serves the artifacts in the team workbench folder (see `serve`) on `port`,
 * or on any free one for `0`, and records the port in `state.workbenchPort`
 * once it is listening. A port it can't bind is logged and left `null`, so
 * the workbench says artifacts can't open rather than the mock failing.
 */
export function startArtifactsListener(
  state: MockState,
  log: (message: string) => void,
  port = 0,
): Server {
  const server = createServer((req, res) => {
    serve(state, req, res);
  });
  server.on("error", (err) => {
    log(`  [mock] Workbench artifacts can't listen on port ${String(port)}: ${err.message}`);
  });
  server.listen(port, "127.0.0.1", () => {
    const address = server.address();
    state.workbenchPort = typeof address === "object" && address !== null ? address.port : null;
    log(`  [mock] Workbench artifacts on http://localhost:${state.workbenchPort}`);
  });
  return server;
}
