import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import type { ServerMessage } from "../src/lib/generated/protocol";
import { apiRoutes } from "./api-routes";
import type { ArtifactsForwarding } from "./artifacts-origin";
import { HUB_STATE_NAME } from "./constants";
import { createMockEnv, type EnvOptions, type MockEnv } from "./env";
import { createHub, mockListing } from "./hub";
import { json } from "./http";
import { createCloudCallbackHandler } from "./cloud";
import { createApiHandler, firstToHandle } from "./middleware";
import { dispatchRoute, type Route } from "./routes";
import { seedAgents } from "./scenario";
import { frameText } from "./sockets";
import { createOverview } from "./overview";
import { createState, type MockAgent, type MockHub, type MockState } from "./state";
import { createTeamEvents } from "./team-events";

export async function advanceMockTime(env: MockEnv, ms: number): Promise<void> {
  if (env.timeMode() !== "manual") env.setTimeMode("manual");
  await env.advance(ms);
}

export function createStubHub(env: MockEnv = createMockEnv()): MockHub {
  const agents = new Map<string, MockAgent>();
  return {
    agents,
    deleted: new Map(),
    appRebuilds: 0,
    env,
    hubState: createState(HUB_STATE_NAME, true, env),
    createAgent(name, options = {}) {
      const agent: MockAgent = {
        name,
        displayName: options.displayName ?? name,
        runState: options.runState ?? "running",
        lastError: null,
        autostart: true,
        role: options.role ?? null,
        visibility: "private",
        busySince: null,
        stopping: false,
        unread: 0,
        state: createState(name, (options.runState ?? "running") === "running", env),
        connectedClients: () => 0,
        watchedPrefixes: () => [],
        receiveMessage: () => undefined,
        dispose: () => undefined,
      };
      agents.set(name, agent);
      return agent;
    },
    summary: (agent) => ({
      name: agent.name,
      display_name: agent.displayName,
      state: agent.runState,
      last_error: agent.lastError,
      autostart: agent.autostart,
      role: agent.role,
      a2a_visibility: agent.visibility,
      teams_configured: false,
    }),
    listing: () => mockListing(agents.values()),
    broadcast: () => {},
    relaySession: () => {},
    lagSessionRelay: () => 0,
    presentPushDevices: () => [],
    teamEvents: createTeamEvents(env, "stub-boot", () => {}),
    overview: createOverview(env, "stub-boot", agents, () => {}),
    setBusy: () => {},
    markStopping: () => {},
    reloadHubConfig: () => {},
    addUnread: () => {},
    clearUnread: () => {},
    transition: () => {},
    setHubSocketOnline: () => {},
    reset: () => {
      env.reset();
    },
  };
}

export const NO_FORWARDING: ArtifactsForwarding = {
  api: () => Promise.resolve(false),
  sockets: null,
};

export function captureFrames(state: MockState): ServerMessage[] {
  const frames: ServerMessage[] = [];
  state.broadcast = (frame) => {
    frames.push(frame);
  };
  return frames;
}

export interface RouteHarness {
  baseUrl: string;
  hub: MockHub;
  state: MockState;
  frames: ServerMessage[];
  close: () => Promise<void>;
}

export async function startRouteHarness(
  routes: readonly Route[],
  env?: MockEnv,
): Promise<RouteHarness> {
  const hub = createStubHub(env);
  const agent = hub.createAgent("atlas");
  const { state } = agent;
  const frames = captureFrames(state);

  const server = createServer((req, res) => {
    const url = new URL(req.url ?? "", "http://localhost");
    void (async () => {
      try {
        const handled = await dispatchRoute(routes, {
          req,
          res,
          hub,
          state,
          method: req.method ?? "GET",
          path: url.pathname,
          query: url.searchParams,
        });
        if (!handled) json(res, 404, { error: `unknown endpoint ${req.method} ${url.pathname}` });
      } catch (err) {
        json(res, 500, { error: err instanceof Error ? err.message : String(err) });
      }
    })();
  });
  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  const { port } = server.address() as AddressInfo;

  return {
    baseUrl: `http://127.0.0.1:${port}`,
    hub,
    state,
    frames,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => {
          resolve();
        });
      }),
  };
}

export async function fetchJson(
  url: string,
  init?: RequestInit,
): Promise<{ status: number; body: unknown }> {
  const res = await fetch(url, init);
  return { status: res.status, body: (await res.json()) as unknown };
}

export async function fetchText(
  url: string,
  init?: RequestInit,
): Promise<{ status: number; body: string }> {
  const res = await fetch(url, init);
  return { status: res.status, body: await res.text() };
}

export type Frame = Record<string, unknown>;

export class TestSocket {
  readonly frames: Frame[] = [];
  readonly closed: Promise<void>;
  private cursor = 0;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly ws: WebSocket) {
    this.closed = new Promise((resolve) => {
      ws.once("close", () => {
        resolve();
      });
    });
    ws.on("message", (raw) => {
      this.frames.push(JSON.parse(frameText(raw)) as Frame);
      for (const listener of this.listeners) listener();
    });
  }

  send(frame: object): void {
    this.ws.send(JSON.stringify(frame));
  }

  sendRaw(text: string): void {
    this.ws.send(text);
  }

  next(match: (frame: Frame) => boolean = () => true, timeoutMs = 3000): Promise<Frame> {
    return new Promise((resolve, reject) => {
      const look = (): boolean => {
        for (let i = this.cursor; i < this.frames.length; i++) {
          const frame = this.frames[i];
          if (frame !== undefined && match(frame)) {
            this.cursor = i + 1;
            resolve(frame);
            return true;
          }
        }
        return false;
      };
      if (look()) return;
      const timer = setTimeout(() => {
        this.listeners.delete(onFrame);
        reject(
          new Error(`no matching frame within ${timeoutMs}ms; saw ${JSON.stringify(this.frames)}`),
        );
      }, timeoutMs);
      const onFrame = (): void => {
        if (look()) {
          clearTimeout(timer);
          this.listeners.delete(onFrame);
        }
      };
      this.listeners.add(onFrame);
    });
  }

  nextOfType(type: string, timeoutMs?: number): Promise<Frame> {
    return this.next((frame) => frame.type === type, timeoutMs);
  }

  async settled(): Promise<Frame[]> {
    const start = this.cursor;
    this.send({ type: "ping" });
    await this.nextOfType("pong");
    return this.frames.slice(start, this.cursor);
  }

  async quietFrames(during?: () => Promise<void>): Promise<Frame[]> {
    const start = this.frames.length;
    await during?.();
    this.cursor = start;
    this.send({ type: "ping" });
    await this.nextOfType("pong");
    return this.frames.slice(start, this.cursor).filter((frame) => frame.type !== "pong");
  }

  close(): Promise<void> {
    this.ws.close();
    return this.closed;
  }
}

export interface MockServerOptions extends EnvOptions {
  seed?: boolean;
  routes?: readonly Route[];
}

export interface MockServerHarness {
  baseUrl: string;
  hub: MockHub;
  openSocket: (path: string) => Promise<TestSocket>;
  refusedUpgrade: (path: string) => Promise<{ status: number; body: unknown }>;
  close: () => Promise<void>;
}

export async function startMockServer(options: MockServerOptions = {}): Promise<MockServerHarness> {
  const server = createServer();
  const hub = createHub(server, {
    env: createMockEnv(options),
    seed: (options.seed ?? true) ? seedAgents : undefined,
  });
  const handle = firstToHandle(
    createApiHandler({ hub, routes: options.routes ?? apiRoutes }),
    createCloudCallbackHandler(hub),
  );
  server.on("request", (req, res) => {
    void handle(req, res).then((handled) => {
      if (!handled) {
        res.writeHead(404);
        res.end();
      }
    });
  });
  await new Promise<void>((resolve) => {
    server.listen(0, "127.0.0.1", resolve);
  });
  const { port } = server.address() as AddressInfo;
  const origin = `127.0.0.1:${port}`;
  const sockets: TestSocket[] = [];

  return {
    baseUrl: `http://${origin}`,
    hub,
    openSocket: (path) =>
      new Promise((resolve, reject) => {
        const ws = new WebSocket(`ws://${origin}${path}`);
        const socket = new TestSocket(ws);
        ws.once("open", () => {
          sockets.push(socket);
          resolve(socket);
        });
        ws.once("error", reject);
        ws.once("unexpected-response", (_req, res) => {
          ws.terminate();
          reject(new Error(`the upgrade was refused with ${res.statusCode}`));
        });
      }),
    refusedUpgrade: (path) =>
      new Promise((resolve, reject) => {
        const ws = new WebSocket(`ws://${origin}${path}`);
        ws.once("open", () => {
          ws.close();
          reject(new Error("the upgrade was accepted"));
        });
        ws.once("error", () => undefined);
        ws.once("unexpected-response", (_req, res) => {
          let body = "";
          res.on("data", (chunk: Buffer) => {
            body += chunk.toString();
          });
          res.on("end", () => {
            ws.terminate();
            resolve({ status: res.statusCode ?? 0, body: JSON.parse(body) as unknown });
          });
        });
      }),
    close: async () => {
      await Promise.all(sockets.map((socket) => socket.close()));
      await new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => {
          resolve();
        });
      });
    },
  };
}
