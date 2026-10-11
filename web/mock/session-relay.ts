import type { WebSocket } from "ws";
import type { ServerMessage } from "../src/lib/generated/protocol";
import type { HubClientMessage, HubServerMessage } from "../src/lib/hub-types";
import { sendFrame } from "./sockets";

/** The `session_*` frames that carry a session's own events: the ones the hub relays. */
const SESSION_EVENT_TYPES = [
  "session_started",
  "session_state_changed",
  "session_completed",
  "session_turn_started",
  "session_turn_ended",
  "session_tool_call",
  "session_tool_result",
  "session_broadcast_response",
  "session_turn_usage",
  "session_response",
  "session_error",
  "session_message_to_main",
] as const;

/** A frame the agent socket sends for an event of one of its sessions. */
export type SessionEventFrame = Extract<
  ServerMessage,
  { type: (typeof SESSION_EVENT_TYPES)[number] }
>;

const sessionEventTypes: ReadonlySet<string> = new Set(SESSION_EVENT_TYPES);

/** Whether `frame` carries an event of a session, rather than an answer to a command or something else. */
export function isSessionEventFrame(frame: ServerMessage): frame is SessionEventFrame {
  return sessionEventTypes.has(frame.type);
}

/** The address of the session a session event frame is about. */
export function sessionAddressOf(frame: SessionEventFrame): string {
  return frame.type === "session_started" ? frame.session.address : frame.address;
}

/** The client messages that subscribe to sessions, or stop. */
export type SessionSubscriptionRequest = Extract<
  HubClientMessage,
  {
    type:
      | "subscribe_session"
      | "unsubscribe_session"
      | "subscribe_artifact_sessions"
      | "unsubscribe_artifact_sessions";
  }
>;

/** The source label of an artifact's sessions is this prefix and its name. */
const ARTIFACT_LABEL_PREFIX = "artifact:";

/** What one page follows: the addresses on each agent, and the artifacts. */
interface Following {
  sessions: Map<string, Set<string>>;
  artifacts: Set<string>;
}

/**
 * The hub socket's session relay: which pages follow which sessions, and the
 * frames sent to them. Like the backend's, a page follows by what it
 * subscribed to, so an artifact subscription also follows sessions that start
 * later, on any agent; a subscription ends with the page's connection.
 */
export interface SessionRelay {
  /**
   * Apply a subscription message from `page`. A subscribe is acknowledged with
   * `subscribed` once it is active; one that names an unknown agent gets a
   * `notice` and no acknowledgement.
   */
  handle: (page: WebSocket, request: SessionSubscriptionRequest) => void;
  /**
   * Send an event of `agent`'s session to the pages that follow it, as a
   * `session_frame`. `label` is the source label the session started with.
   */
  deliver: (
    pages: Iterable<WebSocket>,
    agent: string,
    frame: SessionEventFrame,
    label: string | null,
  ) => void;
  /** Tell the pages that follow something that frames were lost, as a lagging connection is. The number of pages told. */
  lag: (pages: Iterable<WebSocket>) => number;
}

export function createSessionRelay(isKnownAgent: (name: string) => boolean): SessionRelay {
  const following = new WeakMap<WebSocket, Following>();

  const of = (page: WebSocket): Following => {
    const found = following.get(page);
    if (found !== undefined) return found;
    const created: Following = { sessions: new Map(), artifacts: new Set() };
    following.set(page, created);
    return created;
  };

  const follows = (
    page: WebSocket,
    agent: string,
    frame: SessionEventFrame,
    label: string | null,
  ): boolean => {
    const view = following.get(page);
    if (view === undefined) return false;
    if (view.sessions.get(agent)?.has(sessionAddressOf(frame))) return true;
    return (
      label?.startsWith(ARTIFACT_LABEL_PREFIX) === true &&
      view.artifacts.has(label.slice(ARTIFACT_LABEL_PREFIX.length))
    );
  };

  const followsAnything = (view: Following | undefined): boolean =>
    view !== undefined && (view.sessions.size > 0 || view.artifacts.size > 0);

  return {
    handle(page, request) {
      const view = of(page);
      switch (request.type) {
        case "subscribe_session": {
          if (!isKnownAgent(request.agent)) {
            sendFrame(page, {
              type: "notice",
              level: "warn",
              message: `Couldn't follow sessions on ${JSON.stringify(request.agent)}: no agent has that name.`,
            } satisfies HubServerMessage);
            return;
          }
          const addresses = view.sessions.get(request.agent) ?? new Set<string>();
          addresses.add(request.address);
          view.sessions.set(request.agent, addresses);
          sendFrame(page, {
            type: "subscribed",
            kind: "session",
            agent: request.agent,
            address: request.address,
          } satisfies HubServerMessage);
          return;
        }
        case "unsubscribe_session": {
          const addresses = view.sessions.get(request.agent);
          addresses?.delete(request.address);
          if (addresses?.size === 0) view.sessions.delete(request.agent);
          return;
        }
        case "subscribe_artifact_sessions":
          view.artifacts.add(request.artifact);
          sendFrame(page, {
            type: "subscribed",
            kind: "artifact_sessions",
            artifact: request.artifact,
          } satisfies HubServerMessage);
          return;
        case "unsubscribe_artifact_sessions":
          view.artifacts.delete(request.artifact);
          return;
      }
    },
    deliver(pages, agent, frame, label) {
      for (const page of pages) {
        if (follows(page, agent, frame, label)) {
          sendFrame(page, { type: "session_frame", agent, frame } satisfies HubServerMessage);
        }
      }
    },
    lag(pages) {
      let told = 0;
      for (const page of pages) {
        if (!followsAnything(following.get(page))) continue;
        sendFrame(page, { type: "session_relay_lagged" } satisfies HubServerMessage);
        told++;
      }
      return told;
    },
  };
}
