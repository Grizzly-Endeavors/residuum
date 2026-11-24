import type { IncomingMessage } from "node:http";
import type { Duplex } from "node:stream";
import { WebSocket, type RawData, type WebSocketServer } from "ws";
import type { HubServerMessage, HubWorkspaceFrame } from "../src/lib/hub-types";
import type { ServerMessage } from "../src/lib/generated/protocol";
import { byName } from "./util";

export const HUB_SOCKET_PATH = "/api/hub/ws";

export function agentSocketPath(name: string): string {
  return `/api/agents/${name}/ws`;
}

type UpgradeListener = (req: IncomingMessage, socket: Duplex, head: Buffer) => void;

export interface UpgradeHost {
  on: (event: "upgrade", listener: UpgradeListener) => unknown;
  off: (event: "upgrade", listener: UpgradeListener) => unknown;
}

export function isSocketPath(url: string | undefined, path: string): boolean {
  const target = url ?? "";
  return target === path || target.startsWith(`${path}?`);
}

const claimedUpgrades = new WeakSet<IncomingMessage>();

export function isClaimedUpgrade(req: IncomingMessage): boolean {
  return claimedUpgrades.has(req);
}

export function routeUpgrades(
  host: UpgradeHost | null,
  wss: WebSocketServer,
  path: string,
  conflict?: () => object | null,
): () => void {
  const onUpgrade: UpgradeListener = (req, socket, head) => {
    if (!isSocketPath(req.url, path)) return;
    claimedUpgrades.add(req);
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

export function sendFrame(client: WebSocket, frame: object): void {
  if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify(frame));
}

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

export type WatchSet = readonly string[];

export function normalizeWatchPrefix(prefix: string): string {
  return prefix
    .split("/")
    .filter((segment) => segment !== "" && segment !== ".")
    .join("/");
}

function isWithin(path: string, ancestor: string): boolean {
  return ancestor === "" || path === ancestor || path.startsWith(`${ancestor}/`);
}

export function isWorkspaceFrame(
  frame: ServerMessage | HubServerMessage,
): frame is HubWorkspaceFrame {
  return (
    frame.type === "workspace_changed" ||
    frame.type === "workspace_resync" ||
    frame.type === "workspace_watch_unavailable"
  );
}

export function watchedFrame(watch: WatchSet, frame: HubWorkspaceFrame): HubWorkspaceFrame | null {
  if (frame.type !== "workspace_changed") return watch.length === 0 ? null : frame;
  const changes = frame.changes
    .filter(({ path }) => watch.some((prefix) => isWithin(path, prefix) || isWithin(prefix, path)))
    .sort((a, b) => byName(a.path, b.path));
  return changes.length === 0 ? null : { ...frame, changes };
}
