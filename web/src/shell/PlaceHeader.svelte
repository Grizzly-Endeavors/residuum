<script lang="ts">
  import type { Snippet } from "svelte";
  import { displayState } from "../lib/agent-display-state";
  import { hub } from "../lib/hub.svelte";
  import { StatusDot } from "../lib/ui";

  interface Props {
    title: string;
    agent?: string;
    sub?: string | null;
    children?: Snippet;
    actions?: Snippet;
  }

  let { title, agent, sub, children, actions }: Props = $props();

  const summary = $derived(agent === undefined ? undefined : hub.agent(agent));
</script>

<header class="place-header">
  <h1 class="place-title">
    {#if summary}
      <StatusDot
        state={displayState(summary.state, hub.isStopping(summary.name))}
        working={hub.activityOf(summary.name).busy}
      />
    {/if}
    {title}
  </h1>
  {#if sub}
    <span class="place-sub">{sub}</span>
  {/if}
  {@render children?.()}
  {#if actions}
    <div class="place-actions">{@render actions()}</div>
  {/if}
</header>

<style>
  .place-header {
    display: flex;
    flex: none;
    align-items: baseline;
    gap: var(--space-10);
    min-height: var(--layout-place-header-height);
    padding: var(--space-8) var(--space-24);
    border-bottom: 1px solid var(--color-line-soft);
    background: var(--color-stone-0);
  }

  .place-title {
    display: flex;
    align-self: center;
    align-items: center;
    gap: var(--space-6);
    font-size: var(--font-size-message);
    font-weight: var(--font-weight-semibold);
    --leading: var(--line-height-tight);
  }

  .place-sub {
    align-self: center;
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .place-actions {
    display: flex;
    flex: none;
    align-self: center;
    align-items: center;
    gap: var(--space-4);
    margin-left: auto;
  }

  @media (max-width: 760px) {
    .place-header {
      padding: var(--space-6) var(--space-16);
    }
  }
</style>
