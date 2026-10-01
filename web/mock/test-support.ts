import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { WebSocket } from "ws";
import type { ServerMessage } from "../src/lib/generated/protocol";
import { apiRoutes } from "./api-routes";
import { HUB_STATE_NAME } from "./constants";
import { createMockEnv, type EnvOptions, type MockEnv } from "./env";
import { createHub, mockListing } from "./hub";
import { json } from "./http";
import { createApiHandler } from "./middleware";
import { dispatchRoute, type Route } from "./routes";
import { seedAgents } from "./scenario";
import { frameText } from "./sockets";
import { createOverview } from "./overview";
import { createState, type MockAgent, type MockHub, type MockState } from "./state";
import { createTeamEvents } from "./team-events";
import { sleep } from "./util";

/** A hub with no sockets: agents are plain records, and hub frames go nowhere. */
export function createStubHub(env: MockEnv = createMockEnv()): MockHub {
  const agents = new Map<string, MockAgent>();
  return {
    agents,
    deleted: new Map(),
    env,
    hubState: createState(HUB_STATE_NAME, true, env),
    createAgent(name, options = {}) {
      const agent: MockAgent = {
        name,
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
        dispose: () => undefined,
      };
      agents.set(name, agent);
      return agent;
    },
    summary: (agent) => ({
      name: agent.name,
      state: agent.runState,
      last_error: agent.lastError,
      autostart: agent.autostart,
      role: agent.role,
      a2a_visibility: agent.visibility,
    }),
    listing: () => mockListing(agents.values()),
    broadcast: () => {},
    teamEvents: createTeamEvents(env, "stub-boot", () => {}),
    overview: createOverview(env, "stub-boot", agents, () => {}),
    setBusy: () => {},
    markStopping: () => {},
    reloadHubConfig: () => {},
    addUnread: () => {},
    clearUnread: () => {},
    transition: () => {},
    reset: () => {
      env.reset();
    },
  };
}

/** Point a state's broadcast at a list, so a test can read the frames the mock sent. */
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
  /** The state the harness scopes every request to. */
  state: MockState;
  /** Frames the state broadcast, in order. */
  frames: ServerMessage[];
  close: () => Promise<void>;
}

/**
 * Serve route tables over HTTP the way the mock's middleware does: the
 * request is scoped to one agent's state, matched against the tables, and a
 * handler that throws answers 500.
 */
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

/** A response status with its body parsed as JSON; the caller says what shape to expect. */
export async function fetchJson(
  url: string,
  init?: RequestInit,
): Promise<{ status: number; body: unknown }> {
  const res = await fetch(url, init);
  return { status: res.status, body: (await res.json()) as unknown };
}

/** A response status with its body as text. */
export async function fetchText(
  url: string,
  init?: RequestInit,
): Promise<{ status: number; body: string }> {
  const res = await fetch(url, init);
  return { status: res.status, body: await res.text() };
}

/** A frame a socket received, its shape left for the test to assert. */
export type Frame = Record<string, unknown>;

/** A WebSocket client that keeps every frame it receives. */
export class TestSocket {
  /** Every frame received so far, in order. */
  readonly frames: Frame[] = [];
  /** Settles when the connection has closed, whoever closed it. */
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

  /** Send text that isn't a frame the mock can read. */
  sendRaw(text: string): void {
    this.ws.send(text);
  }

  /**
   * The next frame, after the ones already returned, that satisfies `match`.
   * Frames skipped over stay in `frames`.
   */
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

  /** The next frame of `type`. */
  nextOfType(type: string, timeoutMs?: number): Promise<Frame> {
    return this.next((frame) => frame.type === type, timeoutMs);
  }

  /**
   * Wait until the server has handled everything this agent socket sent so
   * far, which a `ping` answered by a `pong` shows, and return the frames that
   * arrived since the last call. The hub socket doesn't answer a `ping`.
   */
  async settled(): Promise<Frame[]> {
    const start = this.cursor;
    this.send({ type: "ping" });
    await this.nextOfType("pong");
    return this.frames.slice(start, this.cursor);
  }

  /** The frames that arrive in the next `ms` milliseconds, for showing that nothing more comes. */
  async quietFrames(ms = 100): Promise<Frame[]> {
    const start = this.frames.length;
    await sleep(ms);
    this.cursor = this.frames.length;
    return this.frames.slice(start);
  }

  close(): Promise<void> {
    this.ws.close();
    return this.closed;
  }
}

export interface MockServerOptions extends EnvOptions {
  /** Create the mock's agents: scout, atlas, drifter and brittle. On by default. */
  seed?: boolean;
  /** The route tables to serve in place of the mock's own (`apiRoutes`). */
  routes?: readonly Route[];
}

export interface MockServerHarness {
  baseUrl: string;
  hub: MockHub;
  /** Open a WebSocket on the server, like `/api/hub/ws`, and wait for it to connect. */
  openSocket: (path: string) => Promise<TestSocket>;
  /** Try a WebSocket upgrade the server refuses, and report its status and JSON body. */
  refusedUpgrade: (path: string) => Promise<{ status: number; body: unknown }>;
  close: () => Promise<void>;
}

/**
 * The whole mock on a real HTTP server: the hub and its agents with their
 * sockets, and every route table behind scoped routing, the way the Vite
 * plugin wires them.
 */
export async function startMockServer(options: MockServerOptions = {}): Promise<MockServerHarness> {
  const server = createServer();
  const hub = createHub(server, {
    env: createMockEnv(options),
    seed: (options.seed ?? true) ? seedAgents : undefined,
  });
  const handle = createApiHandler({ hub, routes: options.routes ?? apiRoutes });
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
