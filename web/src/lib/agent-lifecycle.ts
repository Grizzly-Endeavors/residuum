// When an agent can be started, stopped or restarted.

import type { AgentState } from "./hub-types";

export type LifecycleAction = "start" | "stop" | "restart";

/** Whether `action` applies to an agent in `state`. */
export function lifecycleApplies(action: LifecycleAction, state: AgentState): boolean {
  if (action === "start") return state === "stopped" || state === "failed";
  if (action === "stop") return state === "running" || state === "starting";
  return state === "running" || state === "failed";
}
