import { router } from "./router.svelte";
import { panelAllowed, type Panel } from "./routes";
import { lookUpNewestRun } from "./sessions.svelte";
import { ws } from "./ws.svelte";

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
