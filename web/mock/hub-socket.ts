import { WebSocketServer, type WebSocket } from "ws";
import type { AgentListResponse, HubServerMessage } from "../src/lib/hub-types";
import type { MockClock } from "./env";
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

/** How long a report of `active: true` holds without another one, as in the backend's `presence::FRESH_FOR`. */
export const PRESENCE_FRESH_MS = 60_000;

/** The hub WebSocket, `/api/hub/ws`: server to client frames, plus `watch_team`, `presence`, `ping` and the session subscriptions from the page. */
export interface HubSocket {
  /** Send a frame to every connected page; a change feed frame goes to the pages that watch what it touches. */
  broadcast: (frame: HubServerMessage) => void;
  /** Send an event of an agent's session to the pages that follow it. `label` is the source label the session started with. */
  relaySession: (agent: string, frame: SessionEventFrame, label: string | null) => void;
  /** Tell the pages that follow sessions that frames were lost. The number of pages told. */
  lagSessionRelay: () => number;
  /** Drop every connected page, as a restart of the hub would. */
  dropClients: () => void;
  /**
   * The push devices that have a window in front of the user: each has a
   * fresh `active: true` report from a page that is still connected, so the
   * hub would send it no push. Sorted.
   */
  presentDevices: () => string[];
  /**
   * Take the socket down (`false`): connected pages are dropped and new
   * connections refused, as when the hub can't be reached. `true` lets pages
   * connect again.
   */
  setOnline: (online: boolean) => void;
}

/** A frame a page sent, read. */
type ClientFrame =
  | { type: "watch_team"; prefixes: WatchSet }
  | { type: "presence"; deviceId: string; active: boolean }
  | { type: "ping" }
  | { type: "subscription"; request: SessionSubscriptionRequest };

/** What a `presence` frame asks, or `UNREADABLE_MESSAGE` when it lacks a device or a state. */
function readPresence(body: JsonObject): ClientFrame | { refusal: string } {
  const { device_id: deviceId, active } = body;
  return typeof deviceId === "string" && typeof active === "boolean"
    ? { type: "presence", deviceId, active }
    : { refusal: UNREADABLE_MESSAGE };
}

/** What a `watch_team` frame asks, or why the hub refuses it, in words for the user. */
function readWatchTeam(body: JsonObject): ClientFrame | { refusal: string } {
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
    ? { type: "watch_team", prefixes: [...new Set(watched.map(normalizeWatchPrefix))] }
    : { refusal: `Couldn't watch the team files: ${problem}.` };
}

/** What a session subscription frame asks, or `UNREADABLE_MESSAGE` when it lacks a field its type needs, or has an unknown type. */
function readSubscription(body: JsonObject): ClientFrame | { refusal: string } {
  const agent = stringField(body, "agent");
  const address = stringField(body, "address");
  const artifact = stringField(body, "artifact");
  switch (body.type) {
    case "subscribe_session":
    case "unsubscribe_session":
      return agent === undefined || address === undefined
        ? { refusal: UNREADABLE_MESSAGE }
        : { type: "subscription", request: { type: body.type, agent, address } };
    case "subscribe_artifact_sessions":
    case "unsubscribe_artifact_sessions":
      return artifact === undefined
        ? { refusal: UNREADABLE_MESSAGE }
        : { type: "subscription", request: { type: body.type, artifact } };
    default:
      return { refusal: UNREADABLE_MESSAGE };
  }
}

/**
 * What a frame from a page asks of the hub, or why the hub refuses it. Like
 * the backend's `handle_client_frame`, a frame it can't read is refused with a
 * warning, and so is a `watch_team` prefix outside `team`.
 */
function readClientFrame(raw: string): ClientFrame | { refusal: string } {
  let body: JsonObject;
  try {
    body = parseJsonObject(raw);
  } catch {
    return { refusal: UNREADABLE_MESSAGE };
  }
  if (body.type === "presence") return readPresence(body);
  if (body.type === "watch_team") return readWatchTeam(body);
  if (body.type === "ping") return { type: "ping" };
  return readSubscription(body);
}

/**
 * Open the hub WebSocket on the HTTP server. A page that connects first gets
 * `hub_boot` with `bootId`, then an `agents_snapshot` of `listing()`. Change
 * feed frames that are broadcast reach only the pages whose `watch_team`
 * prefixes they touch. A page's `presence` reports are kept on `clock`, and
 * end when the page disconnects. A page follows sessions by subscribing, to
 * agents that `isKnownAgent` names.
 */
export function openHubSocket(
  host: UpgradeHost | null,
  bootId: string,
  listing: () => AgentListResponse,
  clock: MockClock,
  isKnownAgent: (name: string) => boolean,
): HubSocket {
  const wss = new WebSocketServer({ noServer: true });
  let online = true;
  routeUpgrades(host, wss, HUB_SOCKET_PATH, () =>
    online ? null : { error: "mock: the hub socket is offline" },
  );
  const relay = createSessionRelay(isKnownAgent);
  /** What each page watches (`watch_team`); a page that never asked watches nothing. */
  const watching = new WeakMap<WebSocket, WatchSet>();
  /** The devices each connected page last reported active (`presence`), and when. */
  const presence = new Map<WebSocket, Map<string, number>>();

  const isFresh = (at: number): boolean => clock.now() - at < PRESENCE_FRESH_MS;

  wss.on("connection", (ws: WebSocket) => {
    const reports = new Map<string, number>();
    presence.set(ws, reports);
    ws.on("close", () => presence.delete(ws));
    sendFrame(ws, { type: "hub_boot", boot_id: bootId } satisfies HubServerMessage);
    sendFrame(ws, {
      type: "system_one_status",
      status: { configured: false, provider: null, model: null, outage: null },
    } satisfies HubServerMessage);
    sendFrame(ws, { type: "agents_snapshot", ...listing() } satisfies HubServerMessage);
    ws.on("message", (raw) => {
      const read = readClientFrame(frameText(raw));
      if ("refusal" in read) {
        sendFrame(ws, {
          type: "notice",
          level: "warn",
          message: read.refusal,
        } satisfies HubServerMessage);
      } else if (read.type === "watch_team") {
        watching.set(ws, read.prefixes);
      } else if (read.type === "ping") {
        // Frames are handled in order, so this also says everything sent
        // before the ping has taken effect.
        sendFrame(ws, { type: "pong" } satisfies HubServerMessage);
      } else if (read.type === "subscription") {
        relay.handle(ws, read.request);
      } else {
        // A device that stopped reporting without saying so has gone stale.
        for (const [device, at] of reports) if (!isFresh(at)) reports.delete(device);
        if (read.active) reports.set(read.deviceId, clock.now());
        else reports.delete(read.deviceId);
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
    presentDevices: () => {
      const present = new Set<string>();
      for (const reports of presence.values()) {
        for (const [device, at] of reports) if (isFresh(at)) present.add(device);
      }
      return [...present].sort();
    },
    setOnline: (next) => {
      online = next;
      if (!online) for (const client of wss.clients) client.terminate();
    },
  };
}
