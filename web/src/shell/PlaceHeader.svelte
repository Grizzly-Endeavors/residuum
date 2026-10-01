<script lang="ts">
  import type { Snippet } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { StatusDot } from "../lib/ui";

  // A place's compact title bar: the place's name, or on an agent's places the
  // agent with its state mark and a line under the name.

  interface Props {
    title: string;
    /** The agent whose place this is; its state mark leads the title. */
    agent?: string;
    /** A quiet line beside the title: the agent's role, or the place within the agent. */
    sub?: string | null;
    /** More beside the title, such as Home's tally of its agents. */
    children?: Snippet;
  }

  let { title, agent, sub, children }: Props = $props();

  const summary = $derived(agent === undefined ? undefined : hub.agent(agent));
</script>

<header class="place-header">
  <h1 class="place-title">
    {#if summary}
      <StatusDot
        state={hub.isStopping(summary.name) ? "stopping" : summary.state}
        working={hub.activityOf(summary.name).busy}
      />
    {/if}
    {title}
  </h1>
  {#if sub}
    <span class="place-sub">{sub}</span>
  {/if}
  {@render children?.()}
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
    line-height: var(--line-height-tight);
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

  @media (max-width: 760px) {
    .place-header {
      padding: var(--space-6) var(--space-16);
    }
  }
</style>
