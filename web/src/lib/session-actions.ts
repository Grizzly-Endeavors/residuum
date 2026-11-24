import type { AppAction } from "./action-registry.svelte";

const SESSION_GROUP = "Session";

export interface SessionActionContext {
  agent: string;
  agentReady: boolean;
  stoppable: boolean;
  stopping: boolean;
  stop: () => void;
}

export function sessionActions(ctx: SessionActionContext): AppAction[] {
  let stopBlocked: string | undefined;
  if (!ctx.agentReady) stopBlocked = `Start ${ctx.agent} first`;
  else if (!ctx.stoppable) stopBlocked = "This session isn't running";
  else if (ctx.stopping) stopBlocked = "It's already stopping";

  return [
    {
      id: "session:stop",
      group: SESSION_GROUP,
      label: "Stop this session",
      icon: "stop",
      command: "stop",
      terms: ["cancel"],
      disabled: stopBlocked,
      run: ctx.stop,
    },
  ];
}
