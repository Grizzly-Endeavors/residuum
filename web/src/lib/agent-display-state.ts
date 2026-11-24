import type { AgentState } from "./hub-types";

export type AgentDisplayState = AgentState | "stopping";

export function displayState(state: AgentState, stopping: boolean): AgentDisplayState {
  return stopping && (state === "running" || state === "starting") ? "stopping" : state;
}
