import type { AgentDisplayState } from "./agent-display-state";

export type LifecycleAction = "start" | "stop" | "restart";

export function lifecycleApplies(action: LifecycleAction, state: AgentDisplayState): boolean {
  if (action === "start") return state === "stopped" || state === "failed";
  if (action === "stop") return state === "running" || state === "starting";
  return state === "running" || state === "failed";
}
