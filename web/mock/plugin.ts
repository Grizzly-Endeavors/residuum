import { resolve } from "node:path";
import type { Plugin } from "vite";
import { readMockOptions, startMock, type MockHost } from "./mock";

/**
 * Vite plugin that mocks all Residuum REST endpoints and WebSocket connections.
 * Activated when VITE_MOCK=1 is set (via `npm run dev:mock`, or
 * `npm run preview:mock` for a production build).
 *
 * State is held in-memory for the duration of the server session.
 * Nothing persists across restarts, and `POST /api/mock/reset` restores the
 * initial scenario without one.
 *
 * The mock serves the multi-agent hub HTTP contract
 * (docs/systems-usage/hub-http.md): `/api/agents/{name}/...`,
 * `/api/hub/...` (lifecycle, hub config, secrets, and `/api/hub/ws`) and
 * `/api/team/...`. Each agent has its own state and its own WebSocket. The
 * hub-level and team-level data (secrets, hub config, team files, workbench)
 * live in one shared state.
 *
 * This is the glue: both Vite servers, dev and preview, start the mock
 * (`mock/mock.ts`) on their HTTP server and middleware. `MOCK_DETERMINISTIC=1`
 * makes it repeatable (see `readMockOptions`).
 */
export function mockServerPlugin(): Plugin {
  const start = (
    host: MockHost & { config: { logger: { info: (message: string) => void } } },
    distDir?: string,
  ): void => {
    startMock(
      host,
      readMockOptions(),
      (message) => {
        host.config.logger.info(message);
      },
      distDir,
    );
  };
  return {
    name: "residuum-mock-server",
    configureServer(server) {
      start(server);
    },
    configurePreviewServer(server) {
      start(server, resolve(server.config.root, server.config.build.outDir));
    },
  };
}
