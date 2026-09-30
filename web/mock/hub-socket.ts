import { WebSocketServer, type WebSocket } from "ws";
import type { HubServerMessage } from "../src/lib/hub-types";
import { parseJsonObject } from "./http";
import {
  frameText,
  routeUpgrades,
  sendFrame,
  watchPrefixProblem,
  type UpgradeHost,
} from "./sockets";

/** What the hub says to a page that sent a frame it couldn't read. */
const UNREADABLE_MESSAGE =
  "Residuum couldn't read a message from this page. Reload the page if team files stop updating.";

/** The hub WebSocket, `/api/hub/ws`: server to client frames only, plus `watch_team`. */
export interface HubSocket {
  /** Send a frame to every connected page. */
  broadcast: (frame: HubServerMessage) => void;
}

/**
 * Why the hub refuses a `watch_team` frame, in words for the user, or `null`
 * when it accepts it. Like the backend's `handle_client_frame`, a frame it
 * can't read, or a prefix outside `team`, is refused with a warning.
 */
function watchTeamRefusal(raw: string): string | null {
  let prefixes: unknown;
  try {
    const body = parseJsonObject(raw);
    prefixes = body.type === "watch_team" ? body.prefixes : undefined;
  } catch {
    return UNREADABLE_MESSAGE;
  }
  if (!Array.isArray(prefixes) || prefixes.some((p) => typeof p !== "string")) {
    return UNREADABLE_MESSAGE;
  }
  const watched = prefixes as string[];
  const outsideTeam = watched.find((p) => p !== "team" && !p.startsWith("team/"));
  if (outsideTeam !== undefined) {
    return `Couldn't watch ${JSON.stringify(outsideTeam)}: team watch paths start with team/, like "team/wiki".`;
  }
  const problem = watched.map((p) => watchPrefixProblem(p)).find((p): p is string => p !== null);
  return problem === undefined ? null : `Couldn't watch the team files: ${problem}.`;
}

/**
 * Open the hub WebSocket on the HTTP server. A page that connects first gets
 * `greeting()`, the frames that bring it up to date. The mock has no team
 * files changing, so no change frames follow a `watch_team`.
 */
export function openHubSocket(
  host: UpgradeHost | null,
  greeting: () => HubServerMessage[],
): HubSocket {
  const wss = new WebSocketServer({ noServer: true });
  routeUpgrades(host, wss, "/api/hub/ws");

  wss.on("connection", (ws: WebSocket) => {
    for (const frame of greeting()) sendFrame(ws, frame);
    ws.on("message", (raw) => {
      const refusal = watchTeamRefusal(frameText(raw));
      if (refusal !== null) {
        sendFrame(ws, {
          type: "notice",
          level: "warn",
          message: refusal,
        } satisfies HubServerMessage);
      }
    });
  });

  return {
    broadcast: (frame) => {
      for (const client of wss.clients) sendFrame(client, frame);
    },
  };
}
