// The chat actions, run as `/name` in the composer or from the palette. Each
// has a plain label and a command, so `/observe` runs "Summarize older
// messages now". They act on the bound agent, and the ones that go to it
// need it running. Each says one thing of its own at most: Summarize and
// Condense say they've started, and the agent's notice says when they're
// done; Reload and Add a note leave it all to the agent's notice, which comes
// at once; Fork this conversation names the session it started, with Open
// session, or says why it couldn't.

import type { AppAction } from "./action-registry.svelte";
import type { AgentDisplayState } from "./agent-display-state";
import type { StartSessionRequest } from "./api";
import { userErrorMessage } from "./errors";
import type { NotificationKind } from "./notifications.svelte";
import type { ToastAction } from "./toast.svelte";
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
  surface: (kind: NotificationKind, message: string, action?: ToastAction) => void;
  /** Start a session of the owner's own on an agent; resolves to its address. */
  startSession: (agent: string, request: StartSessionRequest) => Promise<string>;
  /** Show an agent's session in the context panel. */
  openSession: (agent: string, address: string) => void;
  /** Show the agent's conversation size in the context panel. */
  showConversationSize: (agent: string) => void;
  /** Ask for the text of an inbox note. */
  askForInboxNote: (agent: string) => void;
  /** Open the connection status dialog. */
  showConnectionStatus: () => void;
}

/** Why an action that goes over the agent's connection can't run, or undefined when it can. */
export function needsRunningAgent(ctx: ChatActionContext): string | undefined {
  if (ctx.agent === null) return "Open an agent first";
  if (ctx.state === "stopping") return `${ctx.agent} is stopping`;
  if (ctx.state === "starting") return `${ctx.agent} is still starting`;
  if (ctx.state !== "running") return `Start ${ctx.agent} first`;
  return undefined;
}

/**
 * Fork the agent's conversation into a session that works on `task` while
 * the conversation carries on. Its replies stay in the session.
 */
async function forkIntoSession(ctx: ChatActionContext, agent: string, task: string): Promise<void> {
  if (task === "") {
    ctx.surface(
      "notice",
      'Add the task after /multitask, like "/multitask compare the three quotes".',
    );
    return;
  }
  try {
    const address = await ctx.startSession(agent, { prompt: task, fork: true });
    ctx.surface("system", `Forked into ${address}. Its replies show there, not in this chat.`, {
      label: "Open session",
      onClick: () => {
        ctx.openSession(agent, address);
      },
    });
  } catch (err) {
    ctx.surface("error", userErrorMessage(err, { action: "Couldn't fork the conversation." }));
  }
}

export function chatActions(ctx: ChatActionContext): AppAction[] {
  const agent = ctx.agent ?? "the agent";
  const offline = needsRunningAgent(ctx);
  const base = { group: CHAT_GROUP, hint: ctx.agent ?? undefined } as const;

  const serverCommand = (name: string, started: string) => (): void => {
    ctx.send({ type: "server_command", name, args: null });
    ctx.surface("system", started);
  };

  return [
    {
      ...base,
      id: "chat:observe",
      label: "Summarize older messages now",
      icon: "layers",
      command: "observe",
      disabled: offline,
      run: serverCommand("observe", `${agent} is summarizing older messages…`),
    },
    {
      ...base,
      id: "chat:reflect",
      label: "Condense memories now",
      icon: "memory",
      command: "reflect",
      disabled: offline,
      run: serverCommand("reflect", `${agent} is condensing its memories…`),
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
      },
    },
    {
      ...base,
      id: "chat:multitask",
      label: "Fork this conversation into a session",
      icon: "layers",
      command: "multitask",
      terms: ["background", "task"],
      takesText: true,
      disabled: offline,
      run: (text) => {
        if (ctx.agent !== null) void forkIntoSession(ctx, ctx.agent, text?.trim() ?? "");
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
        ctx.showConnectionStatus();
      },
    },
  ];
}
