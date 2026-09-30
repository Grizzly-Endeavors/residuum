import type { PreviewServer, ViteDevServer } from "vite";
import { apiRoutes } from "./api-routes";
import { startArtifactsListener } from "./artifacts-listener";
import { createMockEnv } from "./env";
import { createHub } from "./hub";
import { apiMiddleware, createApiHandler } from "./middleware";
import { seedAgents } from "./scenario";
import type { MockHub } from "./state";

/** The port the artifacts listener takes in deterministic mode, where a free one would differ between runs. */
export const DETERMINISTIC_ARTIFACTS_PORT = 5180;

/** How the mock runs, read from the environment by `readMockOptions`. */
export interface MockOptions {
  /** `VITE_MOCK_SETUP=1`: start with no agents, as a hub that hasn't been set up. */
  setup: boolean;
  /**
   * `MOCK_DETERMINISTIC=1`: a fixed clock, no delays and a fixed artifacts
   * port, so two runs of the same steps give the same responses.
   */
  deterministic: boolean;
  /** `MOCK_DELAY_SCALE`: how long simulated work takes, `1` being the natural pace. Zero when deterministic, one otherwise. */
  delayScale: number;
  /** `MOCK_ARTIFACTS_PORT`: the artifacts listener's port; `0` takes any free one. */
  artifactsPort: number;
}

function numberFrom(raw: string | undefined): number | undefined {
  const value = Number(raw);
  return raw !== undefined && raw !== "" && Number.isFinite(value) && value >= 0
    ? value
    : undefined;
}

export function readMockOptions(env: NodeJS.ProcessEnv = process.env): MockOptions {
  const deterministic = env.MOCK_DETERMINISTIC === "1";
  return {
    setup: env.VITE_MOCK_SETUP === "1",
    deterministic,
    delayScale: numberFrom(env.MOCK_DELAY_SCALE) ?? (deterministic ? 0 : 1),
    artifactsPort:
      numberFrom(env.MOCK_ARTIFACTS_PORT) ?? (deterministic ? DETERMINISTIC_ARTIFACTS_PORT : 0),
  };
}

/** What the mock needs of a Vite server: the dev server and the preview server both have it. */
export type MockHost = Pick<ViteDevServer | PreviewServer, "httpServer" | "middlewares">;

/**
 * Start the mock on a Vite server: the hub with its agents and sockets, the
 * API behind the server's middleware, and the artifacts listener, which closes
 * with the server. The dev server and the preview server both serve the whole
 * mock this way.
 */
export function startMock(
  host: MockHost,
  options: MockOptions,
  log: (message: string) => void,
): MockHub {
  const hub = createHub(host.httpServer, {
    env: createMockEnv(options),
    // With no agents the web UI shows the setup wizard, and finishing it creates the first one.
    seed: options.setup ? undefined : seedAgents,
  });
  host.middlewares.use(apiMiddleware(createApiHandler({ hub, routes: apiRoutes })));
  const listener = startArtifactsListener(hub.hubState, log, options.artifactsPort);
  host.httpServer?.once("close", () => {
    listener.close();
    listener.closeAllConnections();
  });

  log("");
  log("  [mock] API mock server active");
  log(
    `  [mock] Mode: ${options.setup ? "setup" : "running"} (set VITE_MOCK_SETUP=1 for setup wizard)`,
  );
  log("  [mock] Agents: scout, atlas (running), drifter (stopped), brittle (failed)");
  log("  [mock] Hub WebSocket on /api/hub/ws, agent WebSockets on /api/agents/{name}/ws");
  if (options.deterministic) {
    log(
      `  [mock] Deterministic: fixed clock, delay scale ${String(options.delayScale)}; POST /api/mock/reset restores the scenario`,
    );
  }
  log("");
  return hub;
}
