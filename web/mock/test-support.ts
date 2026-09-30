import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import type { ServerMessage } from "../src/lib/generated/protocol";
import { json } from "./http";
import { dispatchRoute, type Route } from "./routes";
import { createState, type MockAgent, type MockHub, type MockState } from "./state";

/** A hub with no sockets: agents are plain records, and hub frames go nowhere. */
export function createStubHub(): MockHub {
  const agents = new Map<string, MockAgent>();
  return {
    agents,
    deleted: new Map(),
    hubState: createState("hub"),
    createAgent(name, options = {}) {
      const agent: MockAgent = {
        name,
        runState: options.runState ?? "running",
        lastError: null,
        autostart: true,
        role: options.role ?? null,
        visibility: "private",
        busy: false,
        unread: 0,
        state: createState(name),
        connectedClients: () => 0,
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
    broadcast: () => {},
    setBusy: () => {},
    addUnread: () => {},
    clearUnread: () => {},
    transition: () => {},
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
export async function startRouteHarness(routes: readonly Route[]): Promise<RouteHarness> {
  const hub = createStubHub();
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
