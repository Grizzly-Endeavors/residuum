// ── Slash command parser ─────────────────────────────────────────────

import type { ClientMessage, FeedItem, ConnectionStatus } from "./types";

export interface CommandResult {
  handled: boolean;
  feedItem?: FeedItem;
  wsMessage?: ClientMessage;
}

interface CommandContext {
  connectionStatus: ConnectionStatus;
  verbose: boolean;
  nextId: () => number;
}

const HELP_TEXT = `Available commands:
  /help          Show this help message
  /verbose       Toggle tool call visibility
  /status        Show connection status
  /observe       Trigger memory observation
  /reflect       Trigger memory reflection
  /context       Show current project context
  /reload        Reload gateway configuration
  /inbox <text>  Add a message to the inbox`;

export function parseCommand(
  input: string,
  ctx: CommandContext,
): CommandResult | null {
  if (!input.startsWith("/")) return null;

  const parts = input.split(/\s+/);
  const cmd = parts[0].toLowerCase();
  const args = parts.slice(1).join(" ");

  switch (cmd) {
    case "/help":
      return {
        handled: true,
        feedItem: {
          id: ctx.nextId(),
          kind: "notice",
          content: HELP_TEXT,
        },
      };

    case "/verbose":
      return {
        handled: true,
        feedItem: {
          id: ctx.nextId(),
          kind: "notice",
          content: `verbose mode will be ${ctx.verbose ? "disabled" : "enabled"}`,
        },
      };

    case "/status":
      return {
        handled: true,
        feedItem: {
          id: ctx.nextId(),
          kind: "notice",
          content: `connection: ${ctx.connectionStatus}\nverbose: ${ctx.verbose ? "on" : "off"}`,
        },
      };

    case "/observe":
      return {
        handled: true,
        wsMessage: { type: "server_command", name: "observe", args: args || null },
        feedItem: {
          id: ctx.nextId(),
          kind: "system",
          content: "Requesting observation...",
        },
      };

    case "/reflect":
      return {
        handled: true,
        wsMessage: { type: "server_command", name: "reflect", args: args || null },
        feedItem: {
          id: ctx.nextId(),
          kind: "system",
          content: "Requesting reflection...",
        },
      };

    case "/context":
      return {
        handled: true,
        wsMessage: { type: "server_command", name: "context", args: args || null },
        feedItem: {
          id: ctx.nextId(),
          kind: "system",
          content: "Requesting context...",
        },
      };

    case "/reload":
      return {
        handled: true,
        wsMessage: { type: "reload" },
        feedItem: {
          id: ctx.nextId(),
          kind: "system",
          content: "Requesting gateway reload...",
        },
      };

    case "/inbox": {
      if (!args.trim()) {
        return {
          handled: true,
          feedItem: {
            id: ctx.nextId(),
            kind: "error",
            content: "usage: /inbox <message>",
          },
        };
      }
      return {
        handled: true,
        wsMessage: { type: "inbox_add", body: args },
        feedItem: {
          id: ctx.nextId(),
          kind: "notice",
          content: `Added to inbox: ${args}`,
        },
      };
    }

    default:
      return {
        handled: true,
        feedItem: {
          id: ctx.nextId(),
          kind: "error",
          content: `unknown command: ${cmd} (try /help)`,
        },
      };
  }
}
