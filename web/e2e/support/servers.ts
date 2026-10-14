/**
 * Where the suite's mock servers listen. The Playwright config starts them and
 * the helpers read the same values, so nothing else spells a port.
 *
 * The mock's state is global to its server, so every worker gets its own pair:
 * a dev server and a preview server, `WORKER_PORT_STRIDE` ports above the
 * previous worker's. A worker's tests reset and drive only its own mock, which
 * is what lets the suite run on several workers at once.
 *
 * Without `E2E_DEV_PORT` and `E2E_PREVIEW_PORT`, the Playwright process picks a
 * free block of ports for each (`workers * WORKER_PORT_STRIDE` wide) when it
 * loads the config, prints them, and writes them into those variables. Every
 * worker process loads this module again and finds the variables set, so all of
 * them read the same ports instead of choosing their own. That is what lets two
 * suites run at once on one machine, from two worktrees for instance, and keeps
 * the suite clear of `npm run dev:mock` (5173, artifacts on 5180) and
 * `just web-mock-preview` (4173). A developer who wants fixed ports sets the
 * variables; a port that is then taken is an error (the servers start with
 * `--strictPort`), never a server to reuse.
 */
import { execFileSync } from "node:child_process";
import { availableParallelism } from "node:os";

/**
 * Where automatically chosen blocks come from: the dev servers' somewhere in
 * the first range, the preview servers' in the second, so the two never
 * overlap. Both sit below Linux's ephemeral ports (32768 and up), which
 * outgoing connections take at random, and clear of 20000-30000, which the
 * Rust tests' port allocator owns (`src/util/test_ports.rs`).
 */
interface PortRange {
  from: number;
  to: number;
}
const AUTO_DEV_RANGE: PortRange = { from: 10_000, to: 15_000 };
const AUTO_PREVIEW_RANGE: PortRange = { from: 15_000, to: 20_000 };

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

/** The highest port a first worker's servers may sit on, so the last worker's artifacts port is still a port. */
const highestBase = 65535 - ARTIFACTS_PORT_OFFSET - (workerCount - 1) * WORKER_PORT_STRIDE;

/**
 * Runs in a child process, because the config has to load synchronously. Tries
 * random bases in the range until every port of a block (each worker's server
 * and artifacts port) accepts a listener on all interfaces, which is how a
 * holder on any address, IPv4 or IPv6, shows up; prints the base.
 */
const FREE_BLOCK_PROBE = `
const net = require("node:net");
const [from, to, workers, stride, artifactsOffset] = process.argv.slice(1).map(Number);
const listenOn = (port, host) => new Promise((resolve) => {
  const probe = net.createServer();
  probe.once("error", (error) => resolve(error.code === "EAFNOSUPPORT" || error.code === "EADDRNOTAVAIL" ? "unsupported" : "taken"));
  probe.listen({ port, host }, () => probe.close(() => resolve("free")));
});
const isFree = async (port) => {
  const dual = await listenOn(port, "::");
  return (dual === "unsupported" ? await listenOn(port, "0.0.0.0") : dual) === "free";
};
(async () => {
  for (let attempt = 0; attempt < 200; attempt++) {
    const base = from + Math.floor(Math.random() * (to - from - workers * stride));
    let free = true;
    for (let worker = 0; worker < workers && free; worker++) {
      const port = base + worker * stride;
      free = (await isFree(port)) && (await isFree(port + artifactsOffset));
    }
    if (free) {
      console.log(base);
      return;
    }
  }
  process.exit(1);
})();
`;

function freeBlockIn(range: PortRange): number {
  let output: string;
  try {
    output = execFileSync(
      process.execPath,
      [
        "-e",
        FREE_BLOCK_PROBE,
        String(range.from),
        String(range.to),
        String(workerCount),
        String(WORKER_PORT_STRIDE),
        String(ARTIFACTS_PORT_OFFSET),
      ],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
  } catch (error) {
    throw new Error(
      `found no free block of ${String(workerCount * WORKER_PORT_STRIDE)} ports between ${String(range.from)} and ${String(range.to)} for the mock servers. ` +
        "Free some ports, or set E2E_DEV_PORT and E2E_PREVIEW_PORT.",
      { cause: error },
    );
  }
  return Number(output.trim());
}

/** The ports this process chose, so it can announce them once, after both are known. */
const chosen: string[] = [];

/**
 * The first worker's port for the servers `name` sets: the variable when set,
 * else a free block, which is then written into the variable for the worker
 * processes to read.
 */
function portFrom(name: string, range: PortRange): number {
  const raw = process.env[name];
  if (raw === undefined || raw === "") {
    const port = freeBlockIn(range);
    process.env[name] = String(port);
    chosen.push(`${name}=${String(port)}`);
    return port;
  }
  const port = Number(raw);
  if (!Number.isInteger(port) || port < 1 || port > highestBase) {
    throw new Error(
      `${name} must be a port number from 1 to ${String(highestBase)} with ${String(workerCount)} workers, got "${raw}"`,
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

const devBase = portFrom("E2E_DEV_PORT", AUTO_DEV_RANGE);
const previewBase = portFrom("E2E_PREVIEW_PORT", AUTO_PREVIEW_RANGE);

if (chosen.length > 0) {
  process.stdout.write(
    `e2e mock servers: ${chosen.join(", ")} (${String(workerCount)} workers, ${String(WORKER_PORT_STRIDE)} ports apart)\n`,
  );
}

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
