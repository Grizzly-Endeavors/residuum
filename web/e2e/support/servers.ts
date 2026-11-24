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

const ARTIFACTS_PORT_OFFSET = 7;

const WORKER_PORT_STRIDE = 10;

const MAX_DEFAULT_WORKERS = 6;

export interface MockServer {
  url: string;
  artifactsUrl: string;
}

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

export const workerCount = workersFrom();

const highestBase = 65535 - ARTIFACTS_PORT_OFFSET - (workerCount - 1) * WORKER_PORT_STRIDE;

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

export interface MockServerLaunch {
  name: string;
  listenArgs: string;
  artifactsPort: number;
  listening: RegExp;
}

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

export type WorkerServers = Record<MockServerKind, MockServer>;

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

export function startedWorkerServers(): WorkerServers[] {
  return Array.from({ length: workerCount }, (_, index) => serversOf(index));
}
