import type { Plugin } from "vite";
import { apiRoutes } from "./api-routes";
import { startArtifactsListener } from "./artifacts-listener";
import { createHub } from "./hub";
import { apiMiddleware, createApiHandler } from "./middleware";
import { seedAgents } from "./scenario";

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
 * This is the glue: it creates the hub and its agents, serves the route tables
 * (`mock/api-routes.ts`) on the dev server, and starts the artifacts listener.
 */
export function mockServerPlugin(): Plugin {
  return {
    name: "residuum-mock-server",
    configureServer(server) {
      const log = (message: string): void => {
        server.config.logger.info(message);
      };
      const hub = createHub(server.httpServer);
      const setup = process.env.VITE_MOCK_SETUP === "1";

      // With no agents the web UI shows the setup wizard, and finishing it
      // creates the first one.
      if (!setup) seedAgents(hub);

      server.middlewares.use(apiMiddleware(createApiHandler({ hub, routes: apiRoutes })));
      const listener = startArtifactsListener(hub.hubState, log);
      server.httpServer?.once("close", () => {
        listener.close();
        listener.closeAllConnections();
      });

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
