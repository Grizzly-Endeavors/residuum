import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { extname, resolve } from "node:path";
import { isValidArtifactName } from "./artifact-name";
import {
  forwardApiRequest,
  forwardUpgrade,
  isApiTarget,
  type ArtifactsForwarding,
} from "./artifacts-origin";
import { WEB_ROOT } from "./assets";
import { keepIdleConnectionsOpen } from "./http";
import { MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./constants";
import type { MockState } from "./state";
import { discoverArtifacts, readArtifactFile } from "./workbench-files";

function findTag(lower: string, tag: string): number {
  const needle = `<${tag}`;
  for (let at = lower.indexOf(needle); at !== -1; at = lower.indexOf(needle, at + needle.length)) {
    if (/[\s>/]/.test(lower[at + needle.length] ?? "")) return at;
  }
  return -1;
}

function embedAsScriptJson(value: unknown): string {
  return JSON.stringify(value).replaceAll("</", "<\\/");
}

export function workbenchPage(html: string, artifactName: string): string {
  const sdk = readFileSync(resolve(WEB_ROOT, "..", "assets", "workbench", "sdk.js"), "utf-8");
  const context =
    `const __RESIDUUM_ARTIFACT__=${embedAsScriptJson(artifactName)};` +
    `const __RESIDUUM_VERSION__=${embedAsScriptJson(MOCK_RESIDUUM_VERSION)};` +
    `const __RESIDUUM_FEATURES__=${embedAsScriptJson(MOCK_FEATURES)};`;
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

function page(res: ServerResponse, status: number, message: string): void {
  const escaped = message.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
  res.writeHead(status, { "Content-Type": "text/html; charset=utf-8" });
  res.end(
    `<!doctype html><meta charset=utf-8><title>Workbench</title><body style="font:14px sans-serif;color:#a8a29e;background:#12100e;padding:2rem"><p>${escaped}</p></body>`,
  );
}

function decode(part: string): string | null {
  try {
    return decodeURIComponent(part);
  } catch {
    return null;
  }
}

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
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  });
  res.end(type === "text/html" ? workbenchPage(file.content, name) : file.content);
}

export function startArtifactsListener(
  state: MockState,
  log: (message: string) => void,
  forwarding: ArtifactsForwarding,
  port = 0,
): Server {
  const server = createServer((req, res) => {
    if (isApiTarget(req.url)) {
      void forwardApiRequest(forwarding, req, res);
    } else {
      serve(state, req, res);
    }
  });
  keepIdleConnectionsOpen(server);
  server.on("upgrade", (req, socket, head) => {
    if (isApiTarget(req.url)) forwardUpgrade(forwarding, req, socket, head);
    else socket.destroy();
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
