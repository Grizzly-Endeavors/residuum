import { WebSocketServer, type WebSocket } from "ws";
import type { AgentListResponse, HubServerMessage } from "../src/lib/hub-types";
import { parseJsonObject } from "./http";
import {
  frameText,
  HUB_SOCKET_PATH,
  isWorkspaceFrame,
  normalizeWatchPrefix,
  routeUpgrades,
  sendFrame,
  watchedFrame,
  watchPrefixProblem,
  type UpgradeHost,
  type WatchSet,
} from "./sockets";

/** What the hub says to a page that sent a frame it couldn't read. */
const UNREADABLE_MESSAGE =
  "Residuum couldn't read a message from this page. Reload the page if team files stop updating.";

/** The hub WebSocket, `/api/hub/ws`: server to client frames only, plus `watch_team`. */
export interface HubSocket {
  /** Send a frame to every connected page; a change feed frame goes to the pages that watch what it touches. */
  broadcast: (frame: HubServerMessage) => void;
  /** Drop every connected page, as a restart of the hub would. */
  dropClients: () => void;
}

/**
 * What a `watch_team` frame asks the hub to watch, or why the hub refuses it,
 * in words for the user. Like the backend's `handle_client_frame`, a frame it
 * can't read, or a prefix outside `team`, is refused with a warning.
 */
function readWatchTeam(raw: string): { prefixes: WatchSet } | { refusal: string } {
  let prefixes: unknown;
  try {
    const body = parseJsonObject(raw);
    prefixes = body.type === "watch_team" ? body.prefixes : undefined;
  } catch {
    return { refusal: UNREADABLE_MESSAGE };
  }
  if (!Array.isArray(prefixes) || prefixes.some((p) => typeof p !== "string")) {
    return { refusal: UNREADABLE_MESSAGE };
  }
  const watched = prefixes as string[];
  const outsideTeam = watched.find((p) => p !== "team" && !p.startsWith("team/"));
  if (outsideTeam !== undefined) {
    return {
      refusal: `Couldn't watch ${JSON.stringify(outsideTeam)}: team watch paths start with team/, like "team/wiki".`,
    };
  }
  const problem = watched.map((p) => watchPrefixProblem(p)).find((p): p is string => p !== null);
  return problem === undefined
    ? { prefixes: [...new Set(watched.map(normalizeWatchPrefix))] }
    : { refusal: `Couldn't watch the team files: ${problem}.` };
}

/**
 * Open the hub WebSocket on the HTTP server. A page that connects first gets
 * `hub_boot` with `bootId`, then an `agents_snapshot` of `listing()`. Change
 * feed frames that are broadcast reach only the pages whose `watch_team`
 * prefixes they touch.
 */
export function openHubSocket(
  host: UpgradeHost | null,
  bootId: string,
  listing: () => AgentListResponse,
): HubSocket {
  const wss = new WebSocketServer({ noServer: true });
  routeUpgrades(host, wss, HUB_SOCKET_PATH);
  /** What each page watches (`watch_team`); a page that never asked watches nothing. */
  const watching = new WeakMap<WebSocket, WatchSet>();

  wss.on("connection", (ws: WebSocket) => {
    sendFrame(ws, { type: "hub_boot", boot_id: bootId } satisfies HubServerMessage);
    sendFrame(ws, { type: "agents_snapshot", ...listing() } satisfies HubServerMessage);
    ws.on("message", (raw) => {
      const read = readWatchTeam(frameText(raw));
      if ("prefixes" in read) {
        watching.set(ws, read.prefixes);
        return;
      }
      sendFrame(ws, {
        type: "notice",
        level: "warn",
        message: read.refusal,
      } satisfies HubServerMessage);
    });
  });

  return {
    broadcast: (frame) => {
      for (const client of wss.clients) {
        const sent = isWorkspaceFrame(frame)
          ? watchedFrame(watching.get(client) ?? [], frame)
          : frame;
        if (sent !== null) sendFrame(client, sent);
      }
    },
    dropClients: () => {
      for (const client of wss.clients) client.terminate();
    },
  };
}
