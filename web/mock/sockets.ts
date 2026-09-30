import type { IncomingMessage } from "node:http";
import type { Duplex } from "node:stream";
import { WebSocket, type RawData, type WebSocketServer } from "ws";

/** The WebSocket path of the hub. */
export const HUB_SOCKET_PATH = "/api/hub/ws";

/** The WebSocket path of one agent. */
export function agentSocketPath(name: string): string {
  return `/api/agents/${name}/ws`;
}

type UpgradeListener = (req: IncomingMessage, socket: Duplex, head: Buffer) => void;

/** The part of the HTTP server a socket route needs: its upgrade requests. */
export interface UpgradeHost {
  on: (event: "upgrade", listener: UpgradeListener) => unknown;
  off: (event: "upgrade", listener: UpgradeListener) => unknown;
}

/** Whether an upgrade request's URL is `path`, with or without a query string. */
export function isSocketPath(url: string | undefined, path: string): boolean {
  const target = url ?? "";
  return target === path || target.startsWith(`${path}?`);
}

/**
 * Hand upgrade requests for `path` to `wss`, or answer them `409` with the
 * JSON body `conflict` returns when it returns one. Every other upgrade is
 * left alone, so Vite's HMR socket keeps working. The result stops routing.
 */
export function routeUpgrades(
  host: UpgradeHost | null,
  wss: WebSocketServer,
  path: string,
  conflict?: () => object | null,
): () => void {
  const onUpgrade: UpgradeListener = (req, socket, head) => {
    if (!isSocketPath(req.url, path)) return;
    const refusal = conflict?.() ?? null;
    if (refusal !== null) {
      const body = JSON.stringify(refusal);
      socket.end(
        `HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`,
      );
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => {
      wss.emit("connection", ws, req);
    });
  };
  host?.on("upgrade", onUpgrade);
  return () => {
    host?.off("upgrade", onUpgrade);
  };
}

/** Send a frame to one client, unless its connection is no longer open. */
export function sendFrame(client: WebSocket, frame: object): void {
  if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify(frame));
}

/** The text of a message a client sent. */
export function frameText(raw: RawData): string {
  if (Array.isArray(raw)) return Buffer.concat(raw).toString();
  if (raw instanceof ArrayBuffer) return Buffer.from(raw).toString();
  return raw.toString();
}

/**
 * Why a workspace watch prefix is refused, or `null` when it is fine: the
 * backend's `WatchSet::parse` refuses absolute paths, `..` segments, and
 * backslashes or NUL characters.
 */
export function watchPrefixProblem(prefix: string): string | null {
  const quoted = JSON.stringify(prefix);
  if (prefix.includes("\\") || prefix.includes("\0")) {
    return `can't watch ${quoted}: it contains a character that can't appear in a workspace path`;
  }
  const segments = prefix.split("/");
  if (prefix.startsWith("/") || (segments[0] ?? "").includes(":")) {
    return `can't watch ${quoted}: watch paths are relative to the workspace, like "team/wiki" or "" for everything`;
  }
  if (segments.includes("..")) {
    return `can't watch ${quoted}: watch paths must stay inside the workspace (no "..")`;
  }
  return null;
}
