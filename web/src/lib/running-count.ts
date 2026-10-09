// How much an agent has running, as the rail's Activity row and the Chat's
// header pill show it, so both say the same number.

import { hub } from "./hub.svelte";
import { ws } from "./ws.svelte";

/**
 * The runs `agent` has going: its live sessions and its open tasks on remote
 * agents (what Activity lists under Running now). Zero for an agent that
 * isn't up, whose last-known runs are over, and for any agent but the bound
 * one, whose sessions the page doesn't load.
 */
export function runningCount(agent: string): number {
  if (ws.agent !== agent) return 0;
  const state = hub.displayStateOf(agent);
  if (state !== "running" && state !== "stopping") return 0;
  return ws.sessions.runningCount;
}
