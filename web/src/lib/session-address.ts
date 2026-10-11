// Opening a session from where it is mentioned (a message in a feed, a
// session's spawner). The sessions store finds the run; the router shows it.
// The session may be on any agent, not only the bound one.

import { router } from "./router.svelte";
import { panelAllowed, type Panel } from "./routes";
import { lookUpNewestRun } from "./sessions.svelte";
import { ws } from "./ws.svelte";

/**
 * Open `agent`'s session at `address` in the context panel: `runId` when the
 * caller has it, else the address's newest run. It opens over the current
 * place when that place can show it, else beside the agent's chat. Tells the
 * user when there is no such run.
 */
export async function openSessionByAddress(
  agent: string,
  address: string,
  runId: string | null,
): Promise<void> {
  const target =
    ws.agent === agent
      ? await ws.sessions.resolveRun(address, runId)
      : (runId ?? (await lookUpNewestRun(agent, address)));
  if (target === null) return;
  const panel: Panel = { kind: "session", agent, runId: target };
  if (panelAllowed(router.place, panel)) await router.openPanel(panel);
  else await router.openPlace({ kind: "chat", agent }, { panel });
}
