// The actions a session's message box offers under `/`: the ones that act on
// the session itself. The chat's actions act on the agent's main
// conversation, so a session's box doesn't list them.

import type { AppAction } from "./action-registry.svelte";

const SESSION_GROUP = "Session";

export interface SessionActionContext {
  agent: string;
  /** The agent is running and not stopping, so its session routes answer. */
  agentReady: boolean;
  /** The run is in a state Stop applies to. */
  stoppable: boolean;
  /** A stop is already under way. */
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
