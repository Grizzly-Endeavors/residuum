// The one way the UI opens an agent's Activity, Schedule or Files (or its
// Chat): as the page, or in the context panel beside the agent's Chat.

import { router } from "../lib/router.svelte";
import { agentPlaceLocation, type AgentPlaceKind, type AgentPlaceWhere } from "../lib/routes";

/** Open one of an agent's places (push). Resolves to whether the navigation happened. */
export function openAgentPlace(
  agent: string,
  kind: AgentPlaceKind,
  where: AgentPlaceWhere,
): Promise<boolean> {
  const { place, panel } = agentPlaceLocation(agent, kind, where);
  return router.openPlace(place, panel === null ? {} : { panel });
}
