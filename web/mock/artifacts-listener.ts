import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { resolve } from "node:path";
import { WEB_ROOT } from "./assets";
import { MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./constants";
import type { MockState } from "./state";

/**
 * Serve an artifact page the way the artifacts listener does: SDK injected,
 * with the artifact's name, the mock version, and the mock feature list
 * embedded for `residuum.artifact`, `residuum.version`, and `residuum.features`.
 */
export function workbenchPage(html: string, artifactName: string): string {
  const sdk = readFileSync(resolve(WEB_ROOT, "..", "assets", "workbench", "sdk.js"), "utf-8");
  const context =
    `const __RESIDUUM_ARTIFACT__=${JSON.stringify(artifactName)};` +
    `const __RESIDUUM_VERSION__=${JSON.stringify(MOCK_RESIDUUM_VERSION)};` +
    `const __RESIDUUM_FEATURES__=${JSON.stringify(MOCK_FEATURES)};`;
  return html.replace("<head>", `<head><script>${context}${sdk}</script>`);
}

/**
 * A second origin for artifacts, like the gateway's artifacts listener:
 * `/{artifact}/` serves the artifact's page. It listens on `port`, or on any
 * free one for `0`, and records the port in `state.workbenchPort` once it is
 * listening. A port it can't bind is logged and left `null`, so the workbench
 * says artifacts can't open rather than the mock failing.
 */
export function startArtifactsListener(
  state: MockState,
  log: (message: string) => void,
  port = 0,
): Server {
  const server = createServer((req, res) => {
    const match = /^\/([a-z0-9-]+)\/(\?.*)?$/.exec(req.url ?? "");
    const name = match?.[1] ?? "";
    const artifact = match ? state.workbenchArtifacts.get(name) : undefined;
    if (!artifact) {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end("There's no workbench artifact here.");
      return;
    }
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" });
    res.end(workbenchPage(artifact.html, name));
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
