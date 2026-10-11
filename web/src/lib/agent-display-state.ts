// The state an agent is shown in: the hub's lifecycle state, with a running
// agent whose stop has begun shown as stopping. Every surface that names or
// marks an agent's state reads it from here.

import type { AgentState } from "./hub-types";

export type AgentDisplayState = AgentState | "stopping";

/**
 * How to show an agent in `state`. `stopping` is whether its name is in the
 * hub's stopping set: listed in the snapshot, or announced by `agent_stopping`
 * with no `agent_state` since. Only an agent that is still up can be stopping;
 * a stale entry for one that has already stopped or failed shows that state.
 */
export function displayState(state: AgentState, stopping: boolean): AgentDisplayState {
  return stopping && (state === "running" || state === "starting") ? "stopping" : state;
}
