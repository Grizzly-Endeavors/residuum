/**
 * Where the suite's mock servers listen. The Playwright config starts them and
 * the helpers read the same values, so nothing else spells a port.
 *
 * The mock's state is global to its server, so every worker gets its own pair:
 * a dev server and a preview server. A worker's tests reset and drive only its
 * own mock, which is what lets the suite run on several workers at once.
 *
 * Each server takes a port that is free when it binds, and its artifacts
 * listener takes any free one. The mock prints both once they listen
 * (`mock/listening.ts`), and Playwright, which waits on that line before it
 * counts a server as started, stores the two ports in its environment under
 * the names `launchOf` gives the pattern's groups. The worker processes start
 * after the servers do and inherit that environment, which is how they find
 * their servers. No port is chosen ahead of the bind, so two suites at once on
 * one machine, from two worktrees for instance, never meet, and a mock you
 * started yourself (`npm run dev:mock` on 5173, `just web-mock-preview` on
 * 4173) can stay up.
 *
 * Vite's dev server can't be told to take any free port (it reads `--port 0`
 * as its default, 5173), so the dev servers ask for `DEV_FIRST_PORT` without
 * `--strictPort`, and Vite moves up one port at a time past the ones taken.
 * The preview servers take port 0. Every server listens on 127.0.0.1 and the
 * tests reach it there, by address rather than by `localhost`, which resolves
 * to `::1` too, where another process can hold the same port.
 *
 * A developer who wants fixed ports sets `E2E_DEV_PORT` and `E2E_PREVIEW_PORT`
 * to the first worker's; each later worker's sit `WORKER_PORT_STRIDE` above
 * the previous one's and each artifacts listener `ARTIFACTS_PORT_OFFSET` above
 * its server. A fixed port that is taken is an error (`--strictPort`), never a
 * server to reuse.
 */
import { availableParallelism } from "node:os";
import { mockListeningPattern } from "../../mock/listening";

/** The address every mock server listens on and the tests reach it at. */
const HOST = "127.0.0.1";

/**
 * Where a dev server starts looking for a free port: below Linux's ephemeral
 * ports (32768 and up), which outgoing connections take at random, and below
 * 20000-30000, which the Rust tests' port allocator owns
 * (`crates/core/src/testing/ports.rs`).
 */
const DEV_FIRST_PORT = 10_000;

/** With fixed ports, how far an artifacts listener sits above its server. */
const ARTIFACTS_PORT_OFFSET = 7;

/** With fixed ports, how far one worker's servers sit above the previous worker's, clear of the artifacts listener. */
const WORKER_PORT_STRIDE = 10;

/**
 * Six workers bring the suite to a few minutes and leave the browsers enough
 * of an 8-CPU runner; a smaller machine gets one worker for every two CPUs.
 */
const MAX_DEFAULT_WORKERS = 6;

export interface MockServer {
  /** The server's origin, which is the `baseURL` of the projects that use it. */
  url: string;
  /** The origin of the mock's artifacts listener, on the same host as `url`, as the app builds it. */
  artifactsUrl: string;
}

/** `E2E_WORKERS`, or the default for this machine. */
function workersFrom(): number {
  const raw = process.env.E2E_WORKERS;
  if (raw === undefined || raw === "") {
    return Math.max(1, Math.min(MAX_DEFAULT_WORKERS, Math.floor(availableParallelism() / 2)));
  }
  const workers = Number(raw);
  if (!Number.isInteger(workers) || workers < 1) {
    throw new Error(`E2E_WORKERS must be a whole number of at least 1, got "${raw}"`);
  }
  return workers;
}

/** How many workers the suite runs on, each with its own mock servers. */
export const workerCount = workersFrom();

/** The highest port a first worker's servers may sit on, so the last worker's artifacts port is still a port. */
const highestBase = 65535 - ARTIFACTS_PORT_OFFSET - (workerCount - 1) * WORKER_PORT_STRIDE;

/** The first worker's fixed port from the variable `name`, or `null` when it isn't set. */
function fixedPortFrom(name: string): number | null {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return null;
  const port = Number(raw);
  if (!Number.isInteger(port) || port < 1 || port > highestBase) {
    throw new Error(
      `${name} must be a port number from 1 to ${String(highestBase)} with ${String(workerCount)} workers, got "${raw}"`,
    );
  }
  return port;
}

/** Which of a worker's servers a project drives. */
export type MockServerKind = "dev" | "preview";

const fixedBase: Record<MockServerKind, number | null> = {
  dev: fixedPortFrom("E2E_DEV_PORT"),
  preview: fixedPortFrom("E2E_PREVIEW_PORT"),
};

/** The environment variables Playwright stores a started server's ports in. */
function portVariables(kind: MockServerKind, index: number): { port: string; artifacts: string } {
  const stem = `E2E_MOCK_${kind.toUpperCase()}_${String(index)}`;
  return { port: `${stem}_PORT`, artifacts: `${stem}_ARTIFACTS_PORT` };
}

/** How the config starts one server. */
export interface MockServerLaunch {
  /** Its name in Playwright's output. */
  name: string;
  /** The arguments that tell Vite where to listen. */
  listenArgs: string;
  /** `MOCK_ARTIFACTS_PORT`. */
  artifactsPort: number;
  /** The line that says it listens, which stores its ports where `serversOf` reads them. */
  listening: RegExp;
}

/** How the config starts the `kind` server of the worker with this `parallelIndex`. */
export function launchOf(kind: MockServerKind, index: number): MockServerLaunch {
  const base = fixedBase[kind];
  const fixed = base === null ? null : base + index * WORKER_PORT_STRIDE;
  const port =
    fixed === null
      ? `--port ${String(kind === "dev" ? DEV_FIRST_PORT : 0)}`
      : `--port ${String(fixed)} --strictPort`;
  const variables = portVariables(kind, index);
  return {
    name: `${kind} ${String(index)}`,
    listenArgs: `--host ${HOST} ${port}`,
    artifactsPort: fixed === null ? 0 : fixed + ARTIFACTS_PORT_OFFSET,
    // Playwright stores each named group under its name in upper case.
    listening: mockListeningPattern(
      variables.port.toLowerCase(),
      variables.artifacts.toLowerCase(),
    ),
  };
}

/** A port Playwright stored for a started server. */
function startedPort(name: string): number {
  const raw = process.env[name] ?? "";
  const port = Number(raw);
  if (raw === "" || !Number.isInteger(port) || port < 1) {
    throw new Error(
      `${name} is "${raw}": Playwright sets it from the mock's "Listening on port" line when it starts the mock servers, so this process started before they did`,
    );
  }
  return port;
}

function startedServer(kind: MockServerKind, index: number): MockServer {
  const variables = portVariables(kind, index);
  const port = startedPort(variables.port);
  return {
    url: `http://${HOST}:${String(port)}`,
    artifactsUrl: `http://${HOST}:${String(startedPort(variables.artifacts))}`,
  };
}

/**
 * One worker's servers: `dev`, the mock on the Vite dev server, which every
 * project but the preview ones uses, and `preview`, the production build served
 * with the mock, for specs tagged `@preview`.
 */
export type WorkerServers = Record<MockServerKind, MockServer>;

/** The servers of the worker with this `parallelIndex`, once Playwright has started them. */
export function serversOf(parallelIndex: number): WorkerServers {
  if (!Number.isInteger(parallelIndex) || parallelIndex < 0 || parallelIndex >= workerCount) {
    throw new Error(
      `no mock servers for worker ${String(parallelIndex)}: the suite starts ${String(workerCount)}`,
    );
  }
  return {
    dev: startedServer("dev", parallelIndex),
    preview: startedServer("preview", parallelIndex),
  };
}

/** Every worker's servers, indexed by the worker's `parallelIndex`, once Playwright has started them. */
export function startedWorkerServers(): WorkerServers[] {
  return Array.from({ length: workerCount }, (_, index) => serversOf(index));
}
