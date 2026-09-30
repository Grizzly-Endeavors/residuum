import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Plugin, ViteDevServer } from "vite";
import type { AgentSummary } from "../src/lib/generated/protocol";
import type { WorkbenchInfo } from "../src/lib/types";
import { mockServerPlugin } from "./plugin";
import { fetchJson } from "./test-support";

type Middleware = (req: IncomingMessage, res: ServerResponse, next: () => void) => void;

/** The part of Vite's dev server the plugin uses, on a real HTTP server. */
interface DevServer {
  http: Server;
  baseUrl: string;
  logged: string[];
}

function startPlugin(): Promise<DevServer> {
  const http = createServer();
  const logged: string[] = [];
  let middleware: Middleware | undefined;
  const server = {
    httpServer: http,
    middlewares: {
      use: (fn: Middleware) => {
        middleware = fn;
      },
    },
    config: {
      logger: {
        info: (message: string) => {
          logged.push(message);
        },
      },
    },
  } as unknown as ViteDevServer;

  const plugin: Plugin = mockServerPlugin();
  const hook = plugin.configureServer;
  const configure = typeof hook === "function" ? hook : hook?.handler;
  if (configure === undefined) throw new Error("the plugin has no configureServer hook");
  void configure.call(undefined as never, server);

  http.on("request", (req, res) => {
    middleware?.(req, res, () => {
      res.writeHead(404);
      res.end();
    });
  });
  return new Promise((resolve) => {
    http.listen(0, "127.0.0.1", () => {
      resolve({
        http,
        baseUrl: `http://127.0.0.1:${(http.address() as AddressInfo).port}`,
        logged,
      });
    });
  });
}

describe("the mock server plugin", () => {
  const running: Server[] = [];

  afterEach(async () => {
    vi.unstubAllEnvs();
    for (const http of running.splice(0)) {
      http.closeAllConnections();
      await new Promise<void>((resolve) => {
        http.close(() => {
          resolve();
        });
      });
    }
  });

  async function start(): Promise<DevServer> {
    const dev = await startPlugin();
    running.push(dev.http);
    return dev;
  }

  it("is named for what it is", () => {
    expect(mockServerPlugin().name).toBe("residuum-mock-server");
  });

  it("serves the mock's agents and says what it is serving", async () => {
    const dev = await start();
    const agents = await fetchJson(`${dev.baseUrl}/api/hub/agents`);
    expect(
      (agents.body as { agents: AgentSummary[] }).agents.map((a) => [a.name, a.state]),
    ).toEqual([
      ["atlas", "running"],
      ["brittle", "failed"],
      ["drifter", "stopped"],
      ["scout", "running"],
    ]);
    expect(dev.logged).toContain("  [mock] API mock server active");
    expect(dev.logged).toContain("  [mock] Mode: running (set VITE_MOCK_SETUP=1 for setup wizard)");
  });

  it("starts in setup mode, with no agents, when VITE_MOCK_SETUP=1", async () => {
    vi.stubEnv("VITE_MOCK_SETUP", "1");
    const dev = await start();
    expect(await fetchJson(`${dev.baseUrl}/api/hub/agents`)).toEqual({
      status: 200,
      body: { agents: [], activity: {}, stopping: [] },
    });
    expect(dev.logged).toContain("  [mock] Mode: setup (set VITE_MOCK_SETUP=1 for setup wizard)");
  });

  it("passes requests outside /api on to the dev server", async () => {
    const dev = await start();
    expect((await fetch(`${dev.baseUrl}/index.html`)).status).toBe(404);
  });

  it("starts the artifacts listener, and reports its port through the workbench info", async () => {
    const dev = await start();
    await vi.waitFor(async () => {
      const info = await fetchJson(`${dev.baseUrl}/api/team/workbench/info`);
      expect((info.body as WorkbenchInfo).port).not.toBeNull();
    });
    const info = (await fetchJson(`${dev.baseUrl}/api/team/workbench/info`)).body as WorkbenchInfo;
    const page = await fetch(`http://127.0.0.1:${info.port}/tip-splitter/`);
    expect(page.status).toBe(200);
    expect(await page.text()).toContain("Tip splitter");
    expect(
      dev.logged.some((line) => line.includes("Workbench artifacts on http://localhost:")),
    ).toBe(true);
  });

  it("closes the artifacts listener with the dev server", async () => {
    const dev = await start();
    let port: number | null = null;
    await vi.waitFor(async () => {
      const info = await fetchJson(`${dev.baseUrl}/api/team/workbench/info`);
      port = (info.body as WorkbenchInfo).port;
      expect(port).not.toBeNull();
    });
    dev.http.closeAllConnections();
    await new Promise<void>((resolve) => {
      dev.http.close(() => {
        resolve();
      });
    });
    await expect(fetch(`http://127.0.0.1:${String(port)}/tip-splitter/`)).rejects.toThrow();
  });
});
