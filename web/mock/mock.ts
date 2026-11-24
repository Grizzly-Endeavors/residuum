import type { Server } from "node:net";
import type { PreviewServer, ViteDevServer } from "vite";
import { apiRoutes } from "./api-routes";
import { appShellPolicy } from "./app-shell-policy";
import { startArtifactsListener } from "./artifacts-listener";
import { createCloudCallbackHandler } from "./cloud";
import { createMockEnv } from "./env";
import { createHub } from "./hub";
import { keepIdleConnectionsOpen } from "./http";
import { mockListeningLine } from "./listening";
import { apiMiddleware, createApiHandler, firstToHandle } from "./middleware";
import { createRebuiltWorkerHandler } from "./rebuilt-worker";
import { seedAgents } from "./scenario";
import type { MockHub } from "./state";

export const DETERMINISTIC_ARTIFACTS_PORT = 5180;

export interface MockOptions {
  setup: boolean;
  deterministic: boolean;
  delayScale: number;
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

function portOnceListening(server: Server, failsOnError: boolean): Promise<number | null> {
  const port = (): number | null => {
    const address = server.address();
    return typeof address === "object" && address !== null ? address.port : null;
  };
  if (server.listening) return Promise.resolve(port());
  return new Promise((resolve) => {
    server.once("listening", () => {
      resolve(port());
    });
    if (failsOnError) {
      server.once("error", () => {
        resolve(null);
      });
    }
  });
}

export type MockHost = Pick<ViteDevServer | PreviewServer, "httpServer" | "middlewares">;

export function startMock(
  host: MockHost,
  options: MockOptions,
  log: (message: string) => void,
  distDir?: string,
): MockHub {
  // An HTTP/2 server, which Vite starts for HTTPS, has no idle timeout to lift; the mock serves plain HTTP.
  if (host.httpServer !== null && "keepAliveTimeout" in host.httpServer) {
    keepIdleConnectionsOpen(host.httpServer);
  }
  const hub = createHub(host.httpServer, {
    env: createMockEnv(options),
    seed: options.setup ? undefined : seedAgents,
  });
  host.middlewares.use(appShellPolicy);
  const api = createApiHandler({ hub, routes: apiRoutes });
  const appOrigin = firstToHandle(api, createCloudCallbackHandler(hub));
  host.middlewares.use(
    apiMiddleware(
      distDir === undefined
        ? appOrigin
        : firstToHandle(appOrigin, createRebuiltWorkerHandler(hub, distDir)),
    ),
  );
  const listener = startArtifactsListener(
    hub.hubState,
    log,
    { api, sockets: host.httpServer },
    options.artifactsPort,
  );
  host.httpServer?.once("close", () => {
    listener.close();
    listener.closeAllConnections();
  });
  if (host.httpServer !== null) {
    // Only `listening` counts for the host: Vite moves on to the next port when one is taken, and each try fails with an `error` first.
    void Promise.all([
      portOnceListening(host.httpServer, false),
      portOnceListening(listener, true),
    ]).then(([port, artifactsPort]) => {
      if (port !== null) log(mockListeningLine(port, artifactsPort));
    });
  }

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
