// ── Slash command parser ─────────────────────────────────────────────

import type { ClientMessage, ConnectionStatus } from "./types";
import type { NotificationKind } from "./notifications.svelte";

export interface CommandNotification {
  kind: NotificationKind;
  message: string;
}

export interface CommandResult {
  handled: boolean;
  /**
   * Output the user should see for this command. Routed through the
   * notification corner (toast + tray history) by the caller.
   *
   * TODO(#98): some commands (e.g. `/help`'s multi-line text) may belong
   * inline rather than as a toast. Audit per command.
   */
  notification?: CommandNotification;
  wsMessage?: ClientMessage;
  /** Side effect to execute after dispatch. */
  action?: () => void;
}

interface CommandContext {
  connectionStatus: ConnectionStatus;
  verbose: boolean;
  setVerbose: (enabled: boolean) => void;
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
        notification: { kind: "notice", message: HELP_TEXT },
      };

    case "/verbose": {
      const newState = !ctx.verbose;
      return {
        handled: true,
        notification: {
          kind: "notice",
          message: `verbose mode will be ${newState ? "enabled" : "disabled"}`,
        },
        action: () => {
          ctx.setVerbose(newState);
        },
      };
    }

    case "/status":
      return {
        handled: true,
        notification: {
          kind: "notice",
          message: `connection: ${ctx.connectionStatus}\nverbose: ${ctx.verbose ? "on" : "off"}`,
        },
      };

    case "/observe":
      return {
        handled: true,
        wsMessage: { type: "server_command", name: "observe", args: args || null },
        notification: { kind: "system", message: "Requesting observation…" },
      };

    case "/reflect":
      return {
        handled: true,
        wsMessage: { type: "server_command", name: "reflect", args: args || null },
        notification: { kind: "system", message: "Requesting reflection…" },
      };

    case "/context":
      return {
        handled: true,
        wsMessage: { type: "server_command", name: "context", args: args || null },
        notification: { kind: "system", message: "Requesting context…" },
      };

    case "/reload":
      return {
        handled: true,
        wsMessage: { type: "reload" },
        notification: { kind: "system", message: "Requesting gateway reload…" },
      };

    case "/inbox": {
      if (!args.trim()) {
        return {
          handled: true,
          notification: { kind: "error", message: "usage: /inbox <message>" },
        };
      }
      return {
        handled: true,
        wsMessage: { type: "inbox_add", body: args },
        notification: { kind: "notice", message: `Added to inbox: ${args}` },
      };
    }

    default:
      return {
        handled: true,
        notification: { kind: "error", message: `unknown command: ${cmd} (try /help)` },
      };
  }
}
