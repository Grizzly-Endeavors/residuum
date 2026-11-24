import type { IncomingMessage, ServerResponse } from "node:http";
import type { Duplex } from "node:stream";
import { json } from "./http";
import { isClaimedUpgrade } from "./sockets";

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

const throughArtifactsOrigin = new WeakSet<IncomingMessage>();

export function arrivedThroughArtifactsOrigin(req: IncomingMessage): boolean {
  return throughArtifactsOrigin.has(req);
}

export type ApiHandler = (req: IncomingMessage, res: ServerResponse) => Promise<boolean>;

export interface UpgradeEmitter {
  emit: (event: "upgrade", req: IncomingMessage, socket: Duplex, head: Buffer) => unknown;
}

export interface ArtifactsForwarding {
  api: ApiHandler;
  sockets: UpgradeEmitter | null;
}

function pathOf(target: string): string {
  const queryAt = target.indexOf("?");
  return queryAt === -1 ? target : target.slice(0, queryAt);
}

export function isApiTarget(target: string | undefined): boolean {
  const path = pathOf(target ?? "");
  return path === "/api" || path.startsWith("/api/");
}

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
