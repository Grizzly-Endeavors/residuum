// Home's agent management: starting, stopping, deleting and creating agents,
// in a disclosure under the board. The rail's "+" opens it from anywhere.

import { tick } from "svelte";

export const agentManagement = $state({ open: false });

/** Open agent management on Home's create form, its name field focused. Call once Home is showing. */
export async function focusAgentCreation(): Promise<void> {
  agentManagement.open = true;
  await tick();
  const name = document.getElementById("create-name");
  name?.scrollIntoView({ block: "center" });
  name?.focus({ preventScroll: true });
}
