<script lang="ts">
  import type { AgentErrorKind } from "../../lib/hub-types";
  import { router } from "../../lib/router.svelte";
  import { Button } from "../../lib/ui";
  import type { ShellActions } from "../../shell/shell-actions";
  import { agentActions } from "./agent-actions.svelte";

  // The fixes for an agent that couldn't start (design §4), shared by Home's
  // needs-you item and the agent's state card: Restart, and by the kind of
  // failure, Fix settings, Open Connections or Report a bug. The fix that
  // matches the failure leads.

  interface Props {
    agent: string;
    kind: AgentErrorKind;
    size?: "sm" | "md";
    actions: ShellActions;
    /** A restart from here is starting. */
    onrestart?: () => void;
    /** A restart from here has finished, whether or not the agent runs now. */
    onrestarted?: () => void;
  }

  let { agent, kind, size = "md", actions, onrestart, onrestarted }: Props = $props();

  const busy = $derived(agentActions.pendingOf(agent));
  const leadsWithFix = $derived(kind === "config" || kind === "port_conflict");

  async function fixSettings(): Promise<void> {
    const section = await agentActions.fixSection(agent);
    if (section !== undefined) await router.openSettings({ scope: agent, section });
  }

  async function restart(): Promise<void> {
    onrestart?.();
    await agentActions.restart(agent);
    onrestarted?.();
  }
</script>

{#if kind === "config"}
  <Button variant="primary" {size} loading={busy === "fix"} onclick={() => void fixSettings()}
    >Fix settings</Button
  >
{:else if kind === "port_conflict"}
  <Button
    variant="primary"
    {size}
    onclick={() => void router.openSettings({ scope: agent, section: "connections" })}
    >Open Connections</Button
  >
{/if}
<Button
  variant={leadsWithFix ? "quiet" : "primary"}
  {size}
  icon="reload"
  loading={busy === "restart"}
  aria-label="Restart {agent}"
  onclick={() => void restart()}>Restart</Button
>
{#if !leadsWithFix}
  <Button variant="quiet" {size} icon="bug" onclick={() => actions.openFeedback("bug")}
    >Report a bug</Button
  >
{/if}
