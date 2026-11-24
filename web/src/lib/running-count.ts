import { hub } from "./hub.svelte";
import { ws } from "./ws.svelte";

export function runningCount(agent: string): number {
  if (ws.agent !== agent) return 0;
  const state = hub.displayStateOf(agent);
  if (state !== "running" && state !== "stopping") return 0;
  return ws.sessions.runningCount;
}
