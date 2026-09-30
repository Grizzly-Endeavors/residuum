import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Plugin } from "vite";
import type { AgentSummary } from "../src/lib/generated/protocol";
import type { WorkbenchInfo } from "../src/lib/types";
import { WebSocket } from "ws";
import { mockServerPlugin } from "./plugin";
import { frameText } from "./sockets";
import { fetchJson } from "./test-support";

type Middleware = (req: IncomingMessage, res: ServerResponse, next: () => void) => void;

/** The part of Vite's dev server the plugin uses, on a real HTTP server. */
interface DevServer {
  http: Server;
  baseUrl: string;
  logged: string[];
}

/** Which Vite server the plugin is started on: the dev server, or the preview server of a production build. */
type ServerKind = "configureServer" | "configurePreviewServer";

function startPlugin(kind: ServerKind = "configureServer"): Promise<DevServer> {
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
  };

  const plugin: Plugin = mockServerPlugin();
  const hook = plugin[kind];
  // The two hooks take different servers, which this fake stands in for both of.
  const configure = (typeof hook === "function" ? hook : hook?.handler) as
    | ((server: unknown) => unknown)
    | undefined;
  if (configure === undefined) throw new Error(`the plugin has no ${kind} hook`);
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

  async function start(kind: ServerKind = "configureServer"): Promise<DevServer> {
    const dev = await startPlugin(kind);
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

  it("serves the same mock from the preview server of a production build", async () => {
    const preview = await start("configurePreviewServer");
    const agents = await fetchJson(`${preview.baseUrl}/api/hub/agents`);
    expect((agents.body as { agents: AgentSummary[] }).agents.map((a) => a.name)).toEqual([
      "atlas",
      "brittle",
      "drifter",
      "scout",
    ]);
    // The preview server's own middleware, the built app, answers what isn't /api.
    expect((await fetch(`${preview.baseUrl}/index.html`)).status).toBe(404);
    await vi.waitFor(async () => {
      const info = (await fetchJson(`${preview.baseUrl}/api/team/workbench/info`))
        .body as WorkbenchInfo;
      expect(info.port).not.toBeNull();
    });
    const frames = await new Promise<string[]>((resolve, reject) => {
      const ws = new WebSocket(`${preview.baseUrl.replace("http", "ws")}/api/hub/ws`);
      const seen: string[] = [];
      ws.on("message", (raw) => {
        seen.push((JSON.parse(frameText(raw)) as { type: string }).type);
        if (seen.length === 2) {
          ws.close();
          resolve(seen);
        }
      });
      ws.on("error", reject);
    });
    expect(frames).toEqual(["hub_boot", "agents_snapshot"]);
  });

  describe("when deterministic", () => {
    /** A port nothing is listening on. */
    async function freePort(): Promise<number> {
      const probe = createServer();
      await new Promise<void>((resolve) => {
        probe.listen(0, "127.0.0.1", resolve);
      });
      const { port } = probe.address() as AddressInfo;
      await new Promise<void>((resolve) => {
        probe.close(() => {
          resolve();
        });
      });
      return port;
    }

    it("listens for artifacts on the port it is given, where a live mock takes a free one", async () => {
      const port = await freePort();
      vi.stubEnv("MOCK_DETERMINISTIC", "1");
      vi.stubEnv("MOCK_ARTIFACTS_PORT", String(port));
      const dev = await start();
      await vi.waitFor(async () => {
        const info = (await fetchJson(`${dev.baseUrl}/api/team/workbench/info`))
          .body as WorkbenchInfo;
        expect(info.port).toBe(port);
      });
      expect(dev.logged.some((line) => line.includes("Deterministic: fixed clock"))).toBe(true);
      expect((await fetch(`http://127.0.0.1:${String(port)}/tip-splitter/`)).status).toBe(200);
    });

    it("says so, and reports artifacts as unavailable, when the port is taken", async () => {
      const taken = createServer();
      await new Promise<void>((resolve) => {
        taken.listen(0, "127.0.0.1", resolve);
      });
      running.push(taken);
      const { port } = taken.address() as AddressInfo;
      vi.stubEnv("MOCK_DETERMINISTIC", "1");
      vi.stubEnv("MOCK_ARTIFACTS_PORT", String(port));
      const dev = await start();
      await vi.waitFor(() => {
        expect(
          dev.logged.some((line) => line.includes(`can't listen on port ${String(port)}`)),
        ).toBe(true);
      });
      const info = (await fetchJson(`${dev.baseUrl}/api/team/workbench/info`))
        .body as WorkbenchInfo;
      expect(info.port).toBeNull();
      expect(info.unavailable_reason).not.toBeNull();
    });

    it("resets through the plugin's own handler", async () => {
      vi.stubEnv("MOCK_DETERMINISTIC", "1");
      vi.stubEnv("MOCK_ARTIFACTS_PORT", String(await freePort()));
      const dev = await start();
      await fetchJson(`${dev.baseUrl}/api/hub/agents/scout`, { method: "DELETE" });
      expect((await fetchJson(`${dev.baseUrl}/api/mock/reset`, { method: "POST" })).body).toEqual({
        ok: true,
      });
      const agents = (await fetchJson(`${dev.baseUrl}/api/hub/agents`)).body as {
        agents: AgentSummary[];
      };
      expect(agents.agents.map((a) => a.name)).toContain("scout");
    });
  });
});
