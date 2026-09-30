/**
 * Vite plugin that mocks all Residuum REST endpoints and WebSocket connections.
 * Activated when VITE_MOCK=1 is set (via `npm run dev:mock`).
 *
 * State is held in-memory for the duration of the dev server session.
 * Nothing persists across restarts.
 *
 * The mock serves the multi-agent hub HTTP contract
 * (docs/systems-usage/hub-http.md): `/api/agents/{name}/...`,
 * `/api/hub/...` (lifecycle, hub config, secrets, and `/api/hub/ws`) and
 * `/api/team/...`. Each agent has its own state and its own WebSocket. The
 * hub-level and team-level data (secrets, hub config, team files, workbench)
 * live in one shared state.
 *
 * The hub, agent sockets, chat, sessions, config and request routing live in
 * `mock/` (see `mock/api-routes.ts` for the route tables). This file creates
 * the agents and serves the requests no route table takes: files, workbench,
 * inbox and test controls.
 */

import type { Plugin } from "vite";
import { createServer, type ServerResponse } from "node:http";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { apiRoutes } from "./mock/api-routes";
import { MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./mock/constants";
import { createHub } from "./mock/hub";
import { json, readBody, text } from "./mock/http";
import { apiMiddleware, createApiHandler } from "./mock/middleware";
import type { RouteRequest } from "./mock/routes";
import { seedAgents } from "./mock/scenario";
import type { MockHub, MockState } from "./mock/state";

// ─── Delays ────────────────────────────────────────────────────────────────────

/**
 * Model calls are slowed down so they're visibly "in flight" in an
 * artifact's activity panel for a moment, long enough to exercise Cancel
 * calls and Stop page by hand.
 */
const MODEL_CALL_DELAY_MS = 3000;

/**
 * Serve an artifact page the way the artifacts listener does: SDK injected,
 * with the artifact's name, the mock version, and the mock feature list
 * embedded for `residuum.artifact`, `residuum.version`, and `residuum.features`.
 */
function workbenchPage(html: string, artifactName: string): string {
  const sdk = readFileSync(resolve(__dirname, "..", "assets", "workbench", "sdk.js"), "utf-8");
  const context =
    `const __RESIDUUM_ARTIFACT__=${JSON.stringify(artifactName)};` +
    `const __RESIDUUM_VERSION__=${JSON.stringify(MOCK_RESIDUUM_VERSION)};` +
    `const __RESIDUUM_FEATURES__=${JSON.stringify(MOCK_FEATURES)};`;
  return html.replace("<head>", `<head><script>${context}${sdk}</script>`);
}

/**
 * A second origin for artifacts, like the gateway's artifacts listener:
 * `/{artifact}/` serves the artifact's page. Listens on any free port and
 * records it.
 */
function startMockArtifactsListener(state: MockState, log: (message: string) => void) {
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
  server.listen(0, "127.0.0.1", () => {
    const address = server.address();
    state.workbenchPort = typeof address === "object" && address !== null ? address.port : null;
    log(`  [mock] Workbench artifacts on http://localhost:${state.workbenchPort}`);
  });
}

// ─── Helpers ───────────────────────────────────────────────────────────────────

// ─── Requests no route table takes ─────────────────────────────────────────────

/** Serve a request no route table matched, and report whether it was one of these. */
async function handleRemaining({
  req,
  res,
  hub,
  state,
  method,
  path,
  query,
}: RouteRequest): Promise<boolean> {
  // Simulate a session's result reaching main while the page is
  // disconnected: record it (and main's reply) in history, then drop
  // the sockets. The page should show it once it reconnects.
  if (path === "/api/mock/missed-relay" && method === "POST") {
    const now = new Date().toISOString();
    state.extraRecent.push(
      {
        role: "user",
        content:
          "[Agent Message from spawned-research-3f9a (spawned)]\nMissed while you were away: the fallback doc is drafted.",
        timestamp: now,
        visibility: "background",
        agent_sender: { address: "spawned-research-3f9a", category: "spawned" },
      },
      {
        role: "assistant",
        content: "The research session finished the fallback doc while you were disconnected.",
        timestamp: now,
        visibility: "background",
      },
    );
    state.dropSockets();
    json(res, 200, { ok: true });
    return true;
  }

  // A teammate messages an agent (`?agent=atlas`): the message lands in its
  // main conversation, and the hub reports it unread until the web UI
  // opens that agent's socket.
  if (path === "/api/mock/teammate-message" && method === "POST") {
    const agent = hub.agents.get(query.get("agent") ?? "");
    if (!agent) {
      json(res, 404, { error: "mock: name an agent with ?agent=" });
      return true;
    }
    const now = new Date().toISOString();
    const from = query.get("from") ?? "scout";
    const reply = `${from} asked me to check the wiki index. On it.`;
    agent.state.extraRecent.push(
      {
        role: "user",
        content: `[Message from ${from}]\nCan you look over the wiki index when you get a chance?`,
        timestamp: now,
        visibility: "user",
      },
      { role: "assistant", content: reply, timestamp: now, visibility: "user" },
    );
    agent.state.broadcast({ type: "response", reply_to: "teammate", content: reply });
    if (agent.connectedClients() === 0) hub.addUnread(agent);
    json(res, 200, { ok: true });
    return true;
  }

  // ── Workbench ─────────────────────────────────────────────────────
  if (path === "/api/workbench/artifacts" && method === "GET") {
    json(
      res,
      200,
      [...state.workbenchArtifacts].map(([name, artifact]) => ({
        name,
        title: /<title>([^<]*)<\/title>/i.exec(artifact.html)?.[1]?.trim() || name,
        modified_at: artifact.modifiedAt,
        size: artifact.html.length,
      })),
    );
    return true;
  }

  if (path === "/api/workbench/info" && method === "GET") {
    json(res, 200, {
      port: state.workbenchPort,
      unavailable_reason:
        state.workbenchPort === null ? "The mock artifacts listener isn't up yet." : null,
      relay: null,
    });
    return true;
  }

  const artifactMatch = path.match(/^\/api\/workbench\/artifacts\/([^/]+)$/);
  if (artifactMatch) {
    const name = decodeURIComponent(artifactMatch[1] ?? "");
    if (method === "DELETE") {
      if (!state.workbenchArtifacts.delete(name)) {
        text(res, 404, "That artifact no longer exists. It may already have been deleted.");
        return true;
      }
      json(res, 200, { removed: [`${name}.html`] });
      return true;
    }
  }

  // ── Model calls ──────────────────────────────────────────────────
  if (path === "/api/model/complete" && method === "POST") {
    const body = JSON.parse(await readBody(req));
    const prompt: string = body.prompt ?? body.messages?.at(-1)?.content ?? "";
    if (!prompt.trim() && !body.messages?.length) {
      json(res, 400, { error: 'A model call needs a "prompt" or "messages".' });
      return true;
    }
    const content = `Mock model reply to: ${prompt.slice(0, 200)}`;
    // A brief artificial delay, so a call is visibly "in flight" in the
    // artifact activity panel long enough to see and, if wanted, cancel.
    await new Promise((done) => setTimeout(done, MODEL_CALL_DELAY_MS));
    json(res, 200, {
      content,
      model: "mock/small",
      usage: { input_tokens: prompt.length, output_tokens: content.length },
    });
    return true;
  }

  return false;
}

// ─── Plugin export ─────────────────────────────────────────────────────────────

export function mockServerPlugin(): Plugin {
  return {
    name: "residuum-mock-server",
    configureServer(server) {
      const log = (message: string) => server.config.logger.info(message);
      const hub = createHub(server.httpServer);
      const setup = process.env.VITE_MOCK_SETUP === "1";

      // With no agents the web UI shows the setup wizard, and finishing it
      // creates the first one.
      if (!setup) seedAgents(hub);

      const handleApi = createApiHandler({ hub, routes: apiRoutes, fallback: handleRemaining });
      server.middlewares.use(apiMiddleware(handleApi));
      startMockArtifactsListener(hub.hubState, log);

      const modeLabel = setup ? "setup" : "running";
      log("");
      log("  [mock] API mock server active");
      log(`  [mock] Mode: ${modeLabel} (set VITE_MOCK_SETUP=1 for setup wizard)`);
      log("  [mock] Agents: scout, atlas (running), drifter (stopped), brittle (failed)");
      log("  [mock] Hub WebSocket on /api/hub/ws, agent WebSockets on /api/agents/{name}/ws");
      log("");
    },
  };
}
