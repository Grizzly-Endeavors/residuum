import { WebSocketServer, type WebSocket } from "ws";
import type { ClientMessage, ServerMessage } from "../src/lib/generated/protocol";
import { createChatSimulator } from "./chat";
import { parseJsonObject, stringField, type JsonObject } from "./http";
import { sendSessionMessage, spawnSession, stopSession } from "./sessions";
import {
  agentSocketPath,
  frameText,
  routeUpgrades,
  sendFrame,
  watchPrefixProblem,
  type UpgradeHost,
} from "./sockets";
import type { MockAgent, MockHub } from "./state";

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
  const verbose = new WeakSet<WebSocket>();

  const stopRouting = routeUpgrades(host, wss, agentSocketPath(agent.name), () =>
    agent.runState === "running"
      ? null
      : { error: `${agent.name} is ${agent.runState}`, state: agent.runState },
  );

  state.dropSockets = () => {
    for (const client of wss.clients) client.terminate();
  };
  state.broadcast = (frame) => {
    for (const client of wss.clients) {
      if (!VERBOSE_ONLY_FRAMES.has(frame.type) || verbose.has(client)) sendFrame(client, frame);
    }
  };
  agent.connectedClients = () => wss.clients.size;
  agent.dispose = () => {
    stopRouting();
    state.dropSockets();
    wss.close();
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

      case "watch_workspace": {
        // The mock has no workspace to watch, so no change frames follow; like
        // the backend, it refuses a prefix it can't use and keeps going.
        const problem = msg.prefixes
          .map((prefix) => watchPrefixProblem(prefix))
          .find((p): p is string => p !== null);
        if (problem !== undefined) {
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

  wss.on("connection", (ws: WebSocket) => {
    // Opening the agent's socket is what shows its messages.
    hub.clearUnread(agent);
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
