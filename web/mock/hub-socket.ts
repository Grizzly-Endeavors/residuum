import { WebSocketServer, type WebSocket } from "ws";
import type { AgentListResponse, HubServerMessage } from "../src/lib/hub-types";
import { parseJsonObject, stringField, type JsonObject } from "./http";
import {
  createSessionRelay,
  type SessionEventFrame,
  type SessionSubscriptionRequest,
} from "./session-relay";
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
  "Residuum couldn't read a message from this page. Reload the page if team files or sessions stop updating.";

/** The hub WebSocket, `/api/hub/ws`: server to client frames, `watch_team` and the session subscriptions. */
export interface HubSocket {
  /** Send a frame to every connected page; a change feed frame goes to the pages that watch what it touches. */
  broadcast: (frame: HubServerMessage) => void;
  /** Send an event of an agent's session to the pages that follow it. `label` is the source label the session started with. */
  relaySession: (agent: string, frame: SessionEventFrame, label: string | null) => void;
  /** Tell the pages that follow sessions that frames were lost. The number of pages told. */
  lagSessionRelay: () => number;
  /** Drop every connected page, as a restart of the hub would. */
  dropClients: () => void;
}

/** What a page's message asks for, or why the hub refuses it, in words for the user. */
type ClientRequest =
  | { watch: WatchSet }
  | { subscription: SessionSubscriptionRequest }
  | { refusal: string };

/**
 * What a `watch_team` frame asks the hub to watch, or why the hub refuses it.
 * Like the backend's `watch_team`, a prefix outside `team` is refused with a warning.
 */
function readWatchTeam(body: JsonObject): ClientRequest {
  const { prefixes } = body;
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
    ? { watch: [...new Set(watched.map(normalizeWatchPrefix))] }
    : { refusal: `Couldn't watch the team files: ${problem}.` };
}

/** A subscription message, which needs the string fields its type names. */
function readSubscription(body: JsonObject): ClientRequest {
  const agent = stringField(body, "agent");
  const address = stringField(body, "address");
  const artifact = stringField(body, "artifact");
  switch (body.type) {
    case "subscribe_session":
    case "unsubscribe_session":
      return agent === undefined || address === undefined
        ? { refusal: UNREADABLE_MESSAGE }
        : { subscription: { type: body.type, agent, address } };
    case "subscribe_artifact_sessions":
    case "unsubscribe_artifact_sessions":
      return artifact === undefined
        ? { refusal: UNREADABLE_MESSAGE }
        : { subscription: { type: body.type, artifact } };
    default:
      return { refusal: UNREADABLE_MESSAGE };
  }
}

/** Read a message a page sent the way the backend's `HubClientMessage` does: a known `type` and its fields. */
function readClientMessage(raw: string): ClientRequest {
  let body: JsonObject;
  try {
    body = parseJsonObject(raw);
  } catch {
    return { refusal: UNREADABLE_MESSAGE };
  }
  return body.type === "watch_team" ? readWatchTeam(body) : readSubscription(body);
}

/**
 * Open the hub WebSocket on the HTTP server. A page that connects first gets
 * `hub_boot` with `bootId`, then an `agents_snapshot` of `listing()`. Change
 * feed frames that are broadcast reach only the pages whose `watch_team`
 * prefixes they touch. A page follows sessions by subscribing, to agents that
 * `isKnownAgent` names.
 */
export function openHubSocket(
  host: UpgradeHost | null,
  bootId: string,
  listing: () => AgentListResponse,
  isKnownAgent: (name: string) => boolean,
): HubSocket {
  const wss = new WebSocketServer({ noServer: true });
  routeUpgrades(host, wss, HUB_SOCKET_PATH);
  const relay = createSessionRelay(isKnownAgent);
  /** What each page watches (`watch_team`); a page that never asked watches nothing. */
  const watching = new WeakMap<WebSocket, WatchSet>();

  wss.on("connection", (ws: WebSocket) => {
    sendFrame(ws, { type: "hub_boot", boot_id: bootId } satisfies HubServerMessage);
    sendFrame(ws, { type: "agents_snapshot", ...listing() } satisfies HubServerMessage);
    ws.on("message", (raw) => {
      const read = readClientMessage(frameText(raw));
      if ("watch" in read) {
        watching.set(ws, read.watch);
      } else if ("subscription" in read) {
        relay.handle(ws, read.subscription);
      } else {
        sendFrame(ws, {
          type: "notice",
          level: "warn",
          message: read.refusal,
        } satisfies HubServerMessage);
      }
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
    relaySession: (agent, frame, label) => {
      relay.deliver(wss.clients, agent, frame, label);
    },
    lagSessionRelay: () => relay.lag(wss.clients),
    dropClients: () => {
      for (const client of wss.clients) client.terminate();
    },
  };
}
