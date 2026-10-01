/**
 * Where the suite's mock servers listen. The Playwright config starts them and
 * the helpers read the same values, so nothing else spells a port.
 *
 * The defaults sit apart from `npm run dev:mock` (5173, artifacts on 5180) and
 * `just web-mock-preview` (4173), so the suite runs while a developer's own mock
 * is up. Two suites at once on one machine, from two worktrees for instance,
 * each set their own `E2E_DEV_PORT` and `E2E_PREVIEW_PORT`.
 */

const DEFAULT_DEV_PORT = 5273;
const DEFAULT_PREVIEW_PORT = 4273;

/** The artifacts listener sits this far above its server's port. */
const ARTIFACTS_PORT_OFFSET = 7;

export interface MockServer {
  /** The port the server listens on. */
  port: number;
  /** The port of the mock's artifacts listener (`MOCK_ARTIFACTS_PORT`). */
  artifactsPort: number;
  /** The server's origin, which is the `baseURL` of the projects that use it. */
  url: string;
}

function portFrom(name: string, fallback: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return fallback;
  const port = Number(raw);
  if (!Number.isInteger(port) || port < 1 || port + ARTIFACTS_PORT_OFFSET > 65535) {
    throw new Error(`${name} must be a port number from 1 to 65528, got "${raw}"`);
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

/** The mock on the Vite dev server, which every project but the preview ones uses. */
export const devServer = serverOn(portFrom("E2E_DEV_PORT", DEFAULT_DEV_PORT));

/** The production build served with the mock, for specs tagged `@preview`. */
export const previewServer = serverOn(portFrom("E2E_PREVIEW_PORT", DEFAULT_PREVIEW_PORT));
