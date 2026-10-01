// The chat actions: what the composer once ran as slash commands. Each has a
// plain label and keeps its old name as its command, so `/observe` still runs
// "Summarize older messages now". They act on the bound agent, and the ones
// that go over its connection need it running.

import type { AppAction } from "./action-registry.svelte";
import type { AgentDisplayState } from "./agent-display-state";
import type { NotificationKind } from "./notifications.svelte";
import type { ClientMessage, ConnectionStatus } from "./types";

export const CHAT_GROUP = "Actions";

export interface ChatActionContext {
  /** The bound agent, or null before there is one. */
  agent: string | null;
  /** Its state as shown, or null when the hub hasn't listed it yet. */
  state: AgentDisplayState | null;
  /** A reply is under way in its chat. */
  replying: boolean;
  hubConnection: ConnectionStatus;
  agentConnection: ConnectionStatus;
  send: (msg: ClientMessage) => void;
  stopReply: () => void;
  surface: (kind: NotificationKind, message: string) => void;
  /** Show the agent's conversation size in the context panel. */
  showConversationSize: (agent: string) => void;
  /** Ask for the text of an inbox note. */
  askForInboxNote: (agent: string) => void;
}

/** Why an action that goes over the agent's connection can't run, or undefined when it can. */
export function needsRunningAgent(ctx: ChatActionContext): string | undefined {
  if (ctx.agent === null) return "Open an agent first";
  if (ctx.state === "stopping") return `${ctx.agent} is stopping`;
  if (ctx.state === "starting") return `${ctx.agent} is still starting`;
  if (ctx.state !== "running") return `Start ${ctx.agent} first`;
  return undefined;
}

/** The hub's and the bound agent's connections, in plain words. */
export function connectionStatusMessage(ctx: ChatActionContext): string {
  const hub =
    ctx.hubConnection === "connected"
      ? "Connected to Residuum."
      : "Can't reach Residuum right now. Trying again.";
  if (ctx.agent === null) return hub;
  let agent: string;
  if (ctx.state !== "running" && ctx.state !== "stopping")
    agent = `${ctx.agent} isn't running, so there's no connection to it.`;
  else if (ctx.agentConnection === "connected") agent = `${ctx.agent} is connected.`;
  else agent = `Reconnecting to ${ctx.agent}. Messages you send will go out once it's back.`;
  return `${hub} ${agent}`;
}

export function chatActions(ctx: ChatActionContext): AppAction[] {
  const agent = ctx.agent ?? "the agent";
  const offline = needsRunningAgent(ctx);
  const base = { group: CHAT_GROUP, hint: ctx.agent ?? undefined } as const;

  const serverCommand = (name: string, notice: string) => (): void => {
    ctx.send({ type: "server_command", name, args: null });
    ctx.surface("system", notice);
  };

  return [
    {
      ...base,
      id: "chat:observe",
      label: "Summarize older messages now",
      icon: "layers",
      command: "observe",
      disabled: offline,
      run: serverCommand("observe", `Asked ${agent} to summarize older messages.`),
    },
    {
      ...base,
      id: "chat:reflect",
      label: "Condense memories now",
      icon: "memory",
      command: "reflect",
      disabled: offline,
      run: serverCommand("reflect", `Asked ${agent} to condense its memories.`),
    },
    {
      ...base,
      id: "chat:context",
      label: "Show conversation size",
      icon: "memory",
      command: "context",
      terms: ["tokens", "context"],
      disabled: ctx.agent === null ? "Open an agent first" : undefined,
      run: () => {
        if (ctx.agent !== null) ctx.showConversationSize(ctx.agent);
      },
    },
    {
      ...base,
      id: "chat:reload",
      label: "Reload settings",
      icon: "reload",
      command: "reload",
      terms: ["config"],
      disabled: offline,
      run: () => {
        ctx.send({ type: "reload" });
        ctx.surface("system", `Asked ${agent} to reload its settings.`);
      },
    },
    {
      ...base,
      id: "chat:stop",
      label: "Stop reply",
      icon: "stop",
      command: "stop",
      terms: ["cancel"],
      disabled: offline ?? (ctx.replying ? undefined : `${agent} isn't replying right now`),
      run: () => {
        ctx.stopReply();
        ctx.surface("system", `Stopping ${agent}'s reply.`);
      },
    },
    {
      ...base,
      id: "chat:inbox",
      label: `Add a note to ${agent}'s inbox`,
      icon: "inbox",
      command: "inbox",
      takesText: true,
      disabled: offline,
      run: (text) => {
        const body = text?.trim() ?? "";
        if (body === "") {
          if (ctx.agent !== null) ctx.askForInboxNote(ctx.agent);
          return;
        }
        ctx.send({ type: "inbox_add", body });
        ctx.surface("notice", `Added a note to ${agent}'s inbox.`);
      },
    },
    {
      ...base,
      id: "chat:status",
      label: "Show connection status",
      icon: "info",
      command: "status",
      terms: ["online", "offline"],
      run: () => {
        ctx.surface("system", connectionStatusMessage(ctx));
      },
    },
  ];
}
