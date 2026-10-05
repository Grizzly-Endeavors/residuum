/**
 * Runs the real workbench SDK (`assets/workbench/sdk.js`) the way an artifact
 * page on the artifacts origin does, against stand-ins for its window: a
 * `fetch` whose requests the test answers by hand, and `FakeWebSocket`s for the
 * hub socket and any agent socket the page opens.
 */
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { vi } from "vitest";
import { FakeWebSocket } from "./fake-websocket";

const SDK_SOURCE = readFileSync(
  new URL("../../../assets/workbench/sdk.js", import.meta.url),
  "utf8",
);

/** The origin the page is served from, in every test. */
export const ARTIFACTS_ORIGIN = "http://localhost:7702";

export interface Frame {
  type: string;
  [key: string]: unknown;
}

export type Handler = (frame: Frame) => void;

export interface SessionHandle {
  agent: string;
  address: string;
  on(type: string, handler: Handler): () => void;
  send(text: string): Promise<string>;
  stop(): Promise<void>;
}

export interface AgentHandle {
  name: string;
  on(type: string, handler: Handler): () => void;
  watch(prefix: string, handler: Handler): () => void;
}

export interface Sdk {
  artifact: string;
  fetch(path: unknown, init?: Record<string, unknown>): Promise<Response>;
  ask(request: unknown, options?: { agent?: string }): Promise<unknown>;
  on(type: unknown, handler: Handler): () => void;
  watch(prefix: string, handler: Handler): () => void;
  agent(name: unknown): AgentHandle;
  state: { get(): Promise<unknown>; set(value: unknown): Promise<void> };
  sessions: {
    start(options: Record<string, unknown>): Promise<SessionHandle>;
    follow(agent: unknown, address: unknown): SessionHandle;
  };
}

/** One request the SDK sent, waiting for the test to answer it. */
export interface HeldRequest {
  url: string;
  method: string;
  headers: Headers;
  body: unknown;
  respond(status: number, body?: unknown): void;
  fail(error: Error): void;
}

export interface LoadedSdk {
  sdk: Sdk;
  /** Every request the SDK sent, oldest first. */
  requests: HeldRequest[];
  /** `location.reload`. */
  reload: ReturnType<typeof vi.fn>;
  /** The page's console, where the SDK reports what it can't tell the page. */
  console: { warn: ReturnType<typeof vi.fn>; error: ReturnType<typeof vi.fn> };
  /** The newest socket the SDK opened at `path`, such as `/api/hub/ws`. */
  socket: (path: string) => FakeWebSocket;
  /** How many sockets the SDK opened at `path`. */
  socketCount: (path: string) => number;
}

function responseOf(status: number, body: unknown): Response {
  if (status === 204) return new Response(null, { status });
  const text = typeof body === "string" ? body : JSON.stringify(body ?? {});
  return new Response(text, {
    status,
    headers: { "content-type": typeof body === "string" ? "text/plain" : "application/json" },
  });
}

/** Load the SDK into a fresh page of artifact `artifact`. */
export function loadSdk(artifact = "chart"): LoadedSdk {
  FakeWebSocket.sockets = [];
  const requests: HeldRequest[] = [];
  const fetch = (url: string, init: RequestInit): Promise<Response> =>
    new Promise((resolve, reject) => {
      requests.push({
        url,
        method: init.method ?? "GET",
        headers: new Headers(init.headers),
        body: init.body,
        respond: (status, body) => {
          resolve(responseOf(status, body));
        },
        fail: reject,
      });
    });
  const reload = vi.fn();
  const pageConsole = { warn: vi.fn(), error: vi.fn(), log: vi.fn() };
  const win: Record<string, unknown> = { fetch };
  runInNewContext(SDK_SOURCE, {
    window: win,
    location: {
      protocol: "http:",
      host: "localhost:7702",
      origin: ARTIFACTS_ORIGIN,
      href: `${ARTIFACTS_ORIGIN}/${artifact}/`,
      reload,
    },
    WebSocket: FakeWebSocket,
    Headers,
    Response,
    URL,
    Blob,
    // Looked up on every call, so a test's fake timers apply.
    setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms),
    clearTimeout: (id?: ReturnType<typeof setTimeout>) => {
      clearTimeout(id);
    },
    setInterval: (fn: () => void, ms?: number) => setInterval(fn, ms),
    clearInterval: (id?: ReturnType<typeof setInterval>) => {
      clearInterval(id);
    },
    queueMicrotask,
    console: pageConsole,
    __RESIDUUM_ARTIFACT__: artifact,
    __RESIDUUM_VERSION__: "2026.09.23",
    __RESIDUUM_FEATURES__: ["artifact-sessions", "workspace-watch"],
  });
  const at = (path: string): FakeWebSocket[] =>
    FakeWebSocket.sockets.filter((s) => new URL(s.url).pathname === path);
  return {
    sdk: win.residuum as Sdk,
    requests,
    reload,
    console: pageConsole,
    socket: (path) => {
      const found = at(path).at(-1);
      if (found === undefined) throw new Error(`no socket was opened at ${path}`);
      return found;
    },
    socketCount: (path) => at(path).length,
  };
}

/** Let the SDK's promise chains and queued microtasks run. */
export async function settle(): Promise<void> {
  for (let i = 0; i < 30; i += 1) await Promise.resolve();
}

/** The newest request the SDK sent. */
export function lastRequest(requests: HeldRequest[]): HeldRequest {
  const last = requests.at(-1);
  if (last === undefined) throw new Error("no request was sent");
  return last;
}

/** A thrown or rejected value's name and message, which survive the SDK's own realm. */
export function described(err: unknown): { name: string; message: string } {
  const { name, message } = err as Error;
  return { name, message };
}

/** What `fn` threw, or a failure when it threw nothing. */
export function thrownBy(fn: () => unknown): { name: string; message: string } {
  try {
    fn();
  } catch (err) {
    return described(err);
  }
  throw new Error("expected a throw");
}
