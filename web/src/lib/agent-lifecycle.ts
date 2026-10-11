// When an agent can be started, stopped or restarted.

import type { AgentDisplayState } from "./agent-display-state";

export type LifecycleAction = "start" | "stop" | "restart";

/**
 * Whether `action` applies to an agent shown in `state`. One that is
 * stopping takes none until it has stopped.
 */
export function lifecycleApplies(action: LifecycleAction, state: AgentDisplayState): boolean {
  if (action === "start") return state === "stopped" || state === "failed";
  if (action === "stop") return state === "running" || state === "starting";
  return state === "running" || state === "failed";
}
