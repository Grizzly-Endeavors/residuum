import type { AppAction } from "./action-registry.svelte";
import type { AgentDisplayState } from "./agent-display-state";
import type { StartSessionRequest } from "./api";
import { userErrorMessage } from "./errors";
import type { NotificationKind } from "./notifications.svelte";
import type { ToastAction } from "./toast.svelte";
import type { ClientMessage, ConnectionStatus } from "./types";

export const CHAT_GROUP = "Actions";

export interface ChatActionContext {
  agent: string | null;
  state: AgentDisplayState | null;
  replying: boolean;
  hubConnection: ConnectionStatus;
  agentConnection: ConnectionStatus;
  send: (msg: ClientMessage) => void;
  stopReply: () => void;
  surface: (kind: NotificationKind, message: string, action?: ToastAction) => void;
  startSession: (agent: string, request: StartSessionRequest) => Promise<string>;
  openSession: (agent: string, address: string) => void;
  showConversationSize: (agent: string) => void;
  composeMailTo: (agent: string) => void;
  showConnectionStatus: () => void;
}

export function needsRunningAgent(ctx: ChatActionContext): string | undefined {
  if (ctx.agent === null) return "Open an agent first";
  if (ctx.state === "stopping") return `${ctx.agent} is stopping`;
  if (ctx.state === "starting") return `${ctx.agent} is still starting`;
  if (ctx.state !== "running") return `Start ${ctx.agent} first`;
  return undefined;
}

export async function forkIntoSession(
  ctx: Pick<ChatActionContext, "surface" | "startSession" | "openSession">,
  agent: string,
  task: string,
): Promise<boolean> {
  try {
    const address = await ctx.startSession(agent, { prompt: task, fork: true });
    ctx.surface("system", `Forked into ${address}. Its replies show there, not in this chat.`, {
      label: "Open session",
      onClick: () => {
        ctx.openSession(agent, address);
      },
    });
    return true;
  } catch (err) {
    ctx.surface("error", userErrorMessage(err, { action: "Couldn't fork the conversation." }));
    return false;
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
      label: `New message to ${agent}`,
      icon: "inbox",
      command: "inbox",
      takesText: true,
      disabled: ctx.agent === null ? offline : undefined,
      disabledWithText: offline,
      run: (text) => {
        const body = text?.trim() ?? "";
        if (body === "") {
          if (ctx.agent !== null) ctx.composeMailTo(ctx.agent);
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
        const task = text?.trim() ?? "";
        if (task === "") {
          ctx.surface(
            "notice",
            'Add the task after /multitask, like "/multitask compare the three quotes".',
          );
          return;
        }
        if (ctx.agent !== null) void forkIntoSession(ctx, ctx.agent, task);
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
