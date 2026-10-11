import type { IncomingMessage, ServerResponse } from "node:http";
import type { Duplex } from "node:stream";
import { json } from "./http";
import { isClaimedUpgrade } from "./sockets";

/**
 * The routes a page on the artifacts origin is refused, whatever the method,
 * as the backend's block list does (`BLOCKED_ROUTES` in
 * `src/hub/http/artifacts_origin.rs`): shutdown, stop-all, update check, apply
 * and restart, and setup completion.
 */
export const ARTIFACT_BLOCKED_ROUTES: readonly string[] = [
  "/api/hub/shutdown",
  "/api/hub/stop-all",
  "/api/hub/update/check",
  "/api/hub/update/apply",
  "/api/hub/update/restart",
  "/api/hub/config/complete-setup",
];

/** The message of a refused call, which the backend sends as `{ error }`. */
export const ARTIFACT_REFUSAL_MESSAGE =
  "Pages opened from the workbench can't shut down, stop, update, or set up Residuum. Do that from the Residuum app.";

/**
 * The requests that came in through the artifacts origin. Membership is the
 * marker, the backend's `ArtifactsOrigin` request extension: only
 * `forwardApiRequest` and `forwardUpgrade` add to it, so nothing a client
 * sends can set it.
 */
const throughArtifactsOrigin = new WeakSet<IncomingMessage>();

/** Whether `req` came in through the artifacts origin. */
export function arrivedThroughArtifactsOrigin(req: IncomingMessage): boolean {
  return throughArtifactsOrigin.has(req);
}

/** The mock's `/api` handler: `true` once it has answered, `false` for a request outside `/api`. */
export type ApiHandler = (req: IncomingMessage, res: ServerResponse) => Promise<boolean>;

/** What the mock's HTTP server needs to hand an upgrade request to its socket routes. */
export interface UpgradeEmitter {
  emit: (event: "upgrade", req: IncomingMessage, socket: Duplex, head: Buffer) => unknown;
}

/** Where the artifacts listener sends `/api`: the mock's API and the server that accepts its sockets, or none when it has no HTTP server. */
export interface ArtifactsForwarding {
  api: ApiHandler;
  sockets: UpgradeEmitter | null;
}

/** A request target without its query. */
function pathOf(target: string): string {
  const queryAt = target.indexOf("?");
  return queryAt === -1 ? target : target.slice(0, queryAt);
}

/** Whether a request target is `/api` or below it, the only paths the listener forwards. */
export function isApiTarget(target: string | undefined): boolean {
  const path = pathOf(target ?? "");
  return path === "/api" || path.startsWith("/api/");
}

/**
 * Answer an `/api` request on the artifacts origin with the mock's API, or
 * refuse it with `403` on the block list.
 */
export async function forwardApiRequest(
  forwarding: ArtifactsForwarding,
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  throughArtifactsOrigin.add(req);
  if (ARTIFACT_BLOCKED_ROUTES.includes(pathOf(req.url ?? ""))) {
    json(res, 403, { error: ARTIFACT_REFUSAL_MESSAGE });
    return;
  }
  try {
    if (!(await forwarding.api(req, res))) json(res, 404, { error: "not found" });
  } catch (err) {
    if (res.headersSent) {
      res.end();
      return;
    }
    json(res, 500, {
      error: `mock server error: ${err instanceof Error ? err.message : String(err)}`,
    });
  }
}

/**
 * Hand an upgrade request on the artifacts origin to the mock's socket
 * routes (the hub socket and the agents'), or refuse it with `404` when none
 * takes it, as the backend does for a path it has no socket at.
 */
export function forwardUpgrade(
  forwarding: ArtifactsForwarding,
  req: IncomingMessage,
  socket: Duplex,
  head: Buffer,
): void {
  throughArtifactsOrigin.add(req);
  forwarding.sockets?.emit("upgrade", req, socket, head);
  if (isClaimedUpgrade(req)) return;
  const body = JSON.stringify({ error: "not found" });
  socket.end(
    `HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`,
  );
}
