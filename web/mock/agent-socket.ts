import type { IncomingMessage } from "node:http";
import { WebSocketServer, type WebSocket } from "ws";
import type { ClientMessage, ServerMessage } from "../src/lib/generated/protocol";
import { arrivedThroughArtifactsOrigin } from "./artifacts-origin";
import { createChatSimulator } from "./chat";
import { parseJsonObject, stringField, type JsonObject } from "./http";
import { sendSessionMessage, spawnSession, stopSession } from "./sessions";
import { isSessionEventFrame } from "./session-relay";
import {
  agentSocketPath,
  frameText,
  isWorkspaceFrame,
  normalizeWatchPrefix,
  routeUpgrades,
  sendFrame,
  watchedFrame,
  watchPrefixProblem,
  type UpgradeHost,
  type WatchSet,
} from "./sockets";
import type { MockAgent, MockHub } from "./state";
import { MockTurnJournal } from "./turn-journal";

/** How long a reload takes before the page is told it finished. */
const RELOAD_MS = 1000;

function requiredString(body: JsonObject, key: string): string {
  const value = stringField(body, key);
  if (value === undefined) throw new Error(`missing or invalid field \`${key}\``);
  return value;
}

function requiredStrings(body: JsonObject, key: string): string[] {
  const value = body[key];
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new Error(`missing or invalid field \`${key}\``);
  }
  return value as string[];
}

/**
 * Read a client frame the way the backend's `ClientMessage` does: a known
 * `type` and the fields that type needs. Anything else throws, and the
 * message says why.
 */
export function parseClientMessage(raw: string): ClientMessage {
  const body = parseJsonObject(raw);
  const type = requiredString(body, "type");
  switch (type) {
    case "send_message":
      return {
        type,
        id: requiredString(body, "id"),
        content: requiredString(body, "content"),
      };
    case "set_verbose": {
      const { enabled } = body;
      if (typeof enabled !== "boolean") throw new Error("missing or invalid field `enabled`");
      return { type, enabled };
    }
    case "ping":
    case "reload":
    case "resync_turn":
      return { type };
    case "server_command":
      return { type, name: requiredString(body, "name"), args: stringField(body, "args") ?? null };
    case "inbox_add":
      return { type, body: requiredString(body, "body") };
    case "cancel":
      return { type, reply_to: requiredString(body, "reply_to") };
    case "session_send_message":
      return {
        type,
        id: requiredString(body, "id"),
        address: requiredString(body, "address"),
        content: requiredString(body, "content"),
      };
    case "session_stop":
      return { type, id: requiredString(body, "id"), address: requiredString(body, "address") };
    case "watch_workspace":
      return { type, prefixes: requiredStrings(body, "prefixes") };
    default:
      throw new Error(`unknown variant \`${type}\``);
  }
}

/** Frames only a client that turned verbose mode on receives: tool calls and results. */
const VERBOSE_ONLY_FRAMES: ReadonlySet<ServerMessage["type"]> = new Set([
  "tool_call",
  "tool_result",
  "session_tool_call",
  "session_tool_result",
]);

/**
 * Open an agent's WebSocket, `/api/agents/{name}/ws`, on the HTTP server. It
 * refuses connections with `409` while the agent isn't running. The agent's
 * state then broadcasts to every connected page and can drop their connections.
 */
export function openAgentSocket(host: UpgradeHost | null, hub: MockHub, agent: MockAgent): void {
  const { state } = agent;
  const wss = new WebSocketServer({ noServer: true });
  const chat = createChatSimulator(hub, agent);
  const journal = new MockTurnJournal(() => hub.env.clock.iso());
  const verbose = new WeakSet<WebSocket>();
  /** The sockets opened through the artifacts origin. A workbench page isn't the user reading the chat, so they don't count as clients. */
  const throughArtifactsOrigin = new WeakSet<WebSocket>();
  /** What each page watches (`watch_workspace`); a page that never asked watches nothing. */
  const watching = new WeakMap<WebSocket, WatchSet>();

  const stopRouting = routeUpgrades(host, wss, agentSocketPath(agent.name), () =>
    agent.runState === "running"
      ? null
      : { error: `${agent.name} is ${agent.runState}`, state: agent.runState },
  );

  state.dropSockets = () => {
    for (const client of wss.clients) client.terminate();
  };
  state.journalOnly = (frame) => {
    journal.record(frame);
  };
  state.broadcast = (frame) => {
    journal.record(frame);
    // The hub's relay carries every session event too, to the pages that follow it.
    if (isSessionEventFrame(frame)) hub.relaySession(agent, frame);
    for (const client of wss.clients) {
      if (VERBOSE_ONLY_FRAMES.has(frame.type) && !verbose.has(client)) continue;
      const sent = isWorkspaceFrame(frame)
        ? watchedFrame(watching.get(client) ?? [], frame)
        : frame;
      if (sent !== null) sendFrame(client, sent);
    }
  };
  agent.receiveMessage = chat.receive;
  agent.connectedClients = () =>
    [...wss.clients].filter((client) => !throughArtifactsOrigin.has(client)).length;
  agent.dispose = () => {
    stopRouting();
    state.dropSockets();
    wss.close();
    journal.clear();
  };

  function handle(ws: WebSocket, msg: ClientMessage): void {
    const reply = (frame: ServerMessage): void => {
      sendFrame(ws, frame);
    };
    switch (msg.type) {
      case "ping":
        reply({ type: "pong" });
        break;

      case "send_message":
        if (msg.content.toLowerCase().startsWith("spawn")) {
          spawnSession(state, msg.content.replace(/^spawn\s*/i, "") || "Look into something");
        }
        chat.send(msg);
        break;

      case "cancel":
        chat.cancel(msg.reply_to);
        break;

      case "set_verbose":
        if (msg.enabled) verbose.add(ws);
        else verbose.delete(ws);
        break;

      case "resync_turn": {
        const turn = journal.snapshot();
        if (turn !== null && !verbose.has(ws)) {
          turn.frames = turn.frames.filter((entry) => !VERBOSE_ONLY_FRAMES.has(entry.frame.type));
        }
        reply({ type: "turn_snapshot", turn });
        break;
      }

      case "watch_workspace": {
        // Like the backend, it replaces what the page watches, unless a prefix
        // can't be used: that is refused and the old set stays.
        const problem = msg.prefixes
          .map((prefix) => watchPrefixProblem(prefix))
          .find((p): p is string => p !== null);
        if (problem === undefined) {
          watching.set(ws, [...new Set(msg.prefixes.map(normalizeWatchPrefix))]);
        } else {
          reply({
            type: "error",
            reply_to: null,
            message: `Couldn't watch the workspace: ${problem}.`,
            details: null,
          });
        }
        break;
      }

      case "session_send_message":
        sendSessionMessage(state, reply, msg.id, msg.address, msg.content);
        break;

      case "session_stop":
        stopSession(state, reply, msg.id, msg.address);
        break;

      case "reload":
        reply({ type: "reloading" });
        hub.env.after(RELOAD_MS, () => {
          reply({ type: "notice", message: "Configuration reloaded successfully." });
        });
        break;

      case "server_command":
        reply({ type: "notice", message: `Command '${msg.name}' executed. (mock)` });
        break;

      case "inbox_add":
        reply({ type: "notice", message: "[inbox] item added" });
        break;
    }
  }

  wss.on("connection", (ws: WebSocket, req: IncomingMessage) => {
    if (arrivedThroughArtifactsOrigin(req)) {
      throughArtifactsOrigin.add(ws);
    } else {
      // Opening the agent's socket is what shows its messages.
      hub.clearUnread(agent);
    }
    ws.on("message", (raw) => {
      let msg: ClientMessage;
      try {
        msg = parseClientMessage(frameText(raw));
      } catch (err) {
        sendFrame(ws, {
          type: "error",
          reply_to: null,
          message: `malformed message: ${err instanceof Error ? err.message : String(err)}`,
          details: null,
        } satisfies ServerMessage);
        return;
      }
      handle(ws, msg);
    });
  });
}
