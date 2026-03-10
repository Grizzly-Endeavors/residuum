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

// ── Command registry ────────────────────────────────────────────────

export interface CommandDef {
  name: string;
  description: string;
  hasArgs: boolean;
}

export const COMMAND_REGISTRY: CommandDef[] = [
  { name: "/help", description: "Show this help message", hasArgs: false },
  { name: "/verbose", description: "Toggle tool call visibility", hasArgs: false },
  { name: "/status", description: "Show connection status", hasArgs: false },
  { name: "/observe", description: "Trigger memory observation", hasArgs: false },
  { name: "/reflect", description: "Trigger memory reflection", hasArgs: false },
  { name: "/context", description: "Show current project context", hasArgs: false },
  { name: "/reload", description: "Reload gateway configuration", hasArgs: false },
  { name: "/inbox", description: "Add a message to the inbox", hasArgs: true },
];

const HELP_TEXT = COMMAND_REGISTRY.map((c) => `  ${c.name.padEnd(15)}${c.description}`).join("\n");

export function filterCommands(query: string): CommandDef[] {
  const q = query.toLowerCase();
  return COMMAND_REGISTRY.filter((c) => c.name.slice(1).startsWith(q));
}

export function parseCommand(input: string, ctx: CommandContext): CommandResult | null {
  if (!input.startsWith("/")) return null;

  const parts = input.split(/\s+/);
  const cmd = (parts[0] ?? "/").toLowerCase();
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
