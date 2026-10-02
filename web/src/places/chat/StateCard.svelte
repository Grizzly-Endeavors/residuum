<script lang="ts">
  import { failureLine } from "../../lib/agent-failure";
  import type { AgentDisplayState } from "../../lib/agent-display-state";
  import { hub } from "../../lib/hub.svelte";
  import type { AgentSummary } from "../../lib/hub-types";
  import { Icon } from "../../lib/icons";
  import { Button, Disclosure, Spinner, Toggle } from "../../lib/ui";
  import type { ShellActions } from "../../shell/shell-actions";
  import { agentActions } from "../home/agent-actions.svelte";
  import FailedAgentActions from "../home/FailedAgentActions.svelte";

  // What an agent's Chat shows in place of the composer while the agent
  // isn't running: why it couldn't start and the fix, that it is
  // stopped with Start, or that it is starting or stopping. The conversation
  // above stays readable.

  interface Props {
    agent: AgentSummary;
    shown: Exclude<AgentDisplayState, "running">;
    /** There's no conversation above it, so it stands alone in the middle of the feed. */
    alone: boolean;
    actions: ShellActions;
  }

  let { agent, shown, alone, actions }: Props = $props();

  const uid = $props.id();
  const name = $derived(agent.name);
  const busy = $derived(agentActions.pendingOf(agent.name));

  let heading = $state<HTMLHeadingElement>();
  /** A restart from this card ended with the agent failed again. */
  let failedAgain = $state(false);

  const title = $derived.by(() => {
    if (shown === "failed") return `${name} couldn't start`;
    if (shown === "stopped") return `${name} is stopped`;
    return shown === "starting" ? `Starting ${name}` : `Stopping ${name}`;
  });

  // The button pressed goes away while the agent starts, so the card's
  // heading keeps the keyboard's place.
  function holdFocus(): void {
    heading?.focus();
  }

  function start(): void {
    holdFocus();
    void agentActions.start(name);
  }

  function restarting(): void {
    holdFocus();
    failedAgain = false;
  }

  function restarted(): void {
    failedAgain = hub.agent(name)?.state === "failed";
  }
</script>

<section class="state-card" class:alone data-state={shown} aria-labelledby="{uid}-title">
  <div class="state-head" role="status">
    <span class="state-mark" aria-hidden="true">
      {#if shown === "starting" || shown === "stopping"}
        <Spinner size={16} />
      {:else}
        <Icon name={shown === "failed" ? "warning" : "pause"} size={18} />
      {/if}
    </span>
    <h2 class="state-title" id="{uid}-title" tabindex="-1" bind:this={heading}>{title}</h2>
    {#if shown === "failed"}
      <p class="state-line">{failureLine(agent.last_error?.kind)}</p>
      {#if failedAgain}
        <p class="state-line state-again">It still couldn't start after the restart.</p>
      {/if}
    {:else if shown === "stopped"}
      <p class="state-line">
        Start it to send messages. Its scheduled work doesn't run while it's stopped.
      </p>
    {:else}
      <p class="state-line">This usually takes a few seconds.</p>
    {/if}
  </div>

  {#if shown === "failed"}
    <div class="state-actions">
      <FailedAgentActions
        agent={name}
        kind={agent.last_error?.kind ?? "other"}
        {actions}
        onrestart={restarting}
        onrestarted={restarted}
      />
    </div>
    {#if agent.last_error}
      <Disclosure summary="Details" tone="quiet">
        <code class="state-reason">{agent.last_error.reason}</code>
      </Disclosure>
    {/if}
  {:else if shown === "stopped"}
    <div class="state-actions">
      <Button variant="primary" icon="play" loading={busy === "start"} onclick={start}>
        Start {name}
      </Button>
      <Toggle
        label="Start automatically"
        layout="inline"
        checked={agentActions.autostartOf(agent)}
        loading={busy === "autostart"}
        onchange={() => void agentActions.toggleAutostart(agent)}
      />
    </div>
  {/if}
</section>

<style>
  .state-card {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-12);
    width: 100%;
    max-width: 480px;
    margin: var(--space-8) auto 0;
    padding: var(--space-20);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);

    &.alone {
      margin-top: 0;
      padding: var(--space-24) var(--space-8);
      background: none;
    }
  }

  .state-head {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-8);
  }

  .state-mark {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    margin-bottom: var(--space-4);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
  }

  .state-card[data-state="failed"] .state-mark {
    background: var(--color-err-tint);
    color: var(--color-err-text);
  }

  .state-card[data-state="starting"] .state-mark {
    color: var(--color-vein-bright);
  }

  .state-title {
    font-size: var(--font-size-heading);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);

    &:focus {
      outline: none;
    }
  }

  .state-line {
    color: var(--color-text-2);
    line-height: var(--line-height-message);
  }

  .state-again {
    color: var(--color-err-text);
  }

  .state-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-16);
    margin-top: var(--space-4);
  }

  .state-reason {
    display: block;
    padding: var(--space-8) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
