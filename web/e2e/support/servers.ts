/**
 * Where the suite's mock servers listen. The Playwright config starts them and
 * the helpers read the same values, so nothing else spells a port.
 *
 * The mock's state is global to its server, so every worker gets its own pair:
 * a dev server and a preview server, `WORKER_PORT_STRIDE` ports above the
 * previous worker's. A worker's tests reset and drive only its own mock, which
 * is what lets the suite run on several workers at once.
 *
 * The defaults sit apart from `npm run dev:mock` (5173, artifacts on 5180) and
 * `just web-mock-preview` (4173), so the suite runs while a developer's own mock
 * is up. Two suites at once on one machine, from two worktrees for instance,
 * each set their own `E2E_DEV_PORT` and `E2E_PREVIEW_PORT`, far enough apart
 * for every worker's ports.
 */
import { availableParallelism } from "node:os";

const DEFAULT_DEV_PORT = 5273;
const DEFAULT_PREVIEW_PORT = 4273;

/** The artifacts listener sits this far above its server's port. */
const ARTIFACTS_PORT_OFFSET = 7;

/** How far one worker's servers sit above the previous worker's, clear of the artifacts listener. */
const WORKER_PORT_STRIDE = 10;

/**
 * Six workers bring the suite to a few minutes and leave the browsers enough
 * of an 8-CPU runner; a smaller machine gets one worker for every two CPUs.
 */
const MAX_DEFAULT_WORKERS = 6;

export interface MockServer {
  /** The port the server listens on. */
  port: number;
  /** The port of the mock's artifacts listener (`MOCK_ARTIFACTS_PORT`). */
  artifactsPort: number;
  /** The server's origin, which is the `baseURL` of the projects that use it. */
  url: string;
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

function portFrom(name: string, fallback: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return fallback;
  const port = Number(raw);
  const highest = 65535 - ARTIFACTS_PORT_OFFSET - (workerCount - 1) * WORKER_PORT_STRIDE;
  if (!Number.isInteger(port) || port < 1 || port > highest) {
    throw new Error(
      `${name} must be a port number from 1 to ${String(highest)} with ${String(workerCount)} workers, got "${raw}"`,
    );
  }
  return port;
}

function serverOn(port: number): MockServer {
  return {
    port,
    artifactsPort: port + ARTIFACTS_PORT_OFFSET,
    url: `http://localhost:${port}`,
  };
}

/** Which of a worker's servers a project drives. */
export type MockServerKind = "dev" | "preview";

/**
 * One worker's servers: `dev`, the mock on the Vite dev server, which every
 * project but the preview ones uses, and `preview`, the production build served
 * with the mock, for specs tagged `@preview`.
 */
export type WorkerServers = Record<MockServerKind, MockServer>;

const devBase = portFrom("E2E_DEV_PORT", DEFAULT_DEV_PORT);
const previewBase = portFrom("E2E_PREVIEW_PORT", DEFAULT_PREVIEW_PORT);

/** Every worker's servers, indexed by the worker's `parallelIndex`. */
export const workerServers: readonly WorkerServers[] = Array.from(
  { length: workerCount },
  (_, index) => ({
    dev: serverOn(devBase + index * WORKER_PORT_STRIDE),
    preview: serverOn(previewBase + index * WORKER_PORT_STRIDE),
  }),
);

/** The servers of the worker with this `parallelIndex`. */
export function serversOf(parallelIndex: number): WorkerServers {
  const servers = workerServers[parallelIndex];
  if (servers === undefined) {
    throw new Error(
      `no mock servers for worker ${String(parallelIndex)}: the suite starts ${String(workerCount)}`,
    );
  }
  return servers;
}
