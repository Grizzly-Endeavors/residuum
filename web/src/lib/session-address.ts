// Opening a session from where it is mentioned (a message in the chat, a
// session's spawner). The sessions store finds the run; the router shows it.

import { router } from "./router.svelte";
import type { SessionsStore } from "./sessions.svelte";

/**
 * Open the session at `address` in the context panel: `runId` when the caller
 * has it, else the address's newest run. Tells the user when there is none.
 */
export async function openSessionByAddress(
  sessions: SessionsStore,
  address: string,
  runId: string | null,
): Promise<void> {
  const agent = sessions.agent;
  const target = await sessions.resolveRun(address, runId);
  if (agent === null || target === null) return;
  await router.openPanel({ kind: "session", agent, runId: target });
}
