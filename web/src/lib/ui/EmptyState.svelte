<script lang="ts">
  import type { Snippet } from "svelte";
  import { Icon, type IconName } from "../icons";

  // What a place or list shows when it has nothing yet, and what to do about
  // it. `line` is one quiet sentence inside a list; `block` stands in for a
  // whole place. A list that failed to load shows a Banner instead.
  interface Props {
    variant?: "line" | "block";
    /** Block only. */
    icon?: IconName;
    /** Block only. */
    title?: string;
    /** The title's heading level, to fit the outline it sits in. */
    headingLevel?: 2 | 3 | 4;
    actions?: Snippet;
    children: Snippet;
  }

  let { variant = "line", icon, title, headingLevel = 3, actions, children }: Props = $props();
</script>

{#if variant === "line"}
  <div class="ui-empty-line">
    <p>{@render children()}</p>
    {@render actions?.()}
  </div>
{:else}
  <div class="ui-empty-block">
    {#if icon}
      <span class="ui-empty-icon"><Icon name={icon} size={18} /></span>
    {/if}
    {#if title}
      <svelte:element this={`h${headingLevel}`} class="ui-empty-title">{title}</svelte:element>
    {/if}
    <p class="ui-empty-text">{@render children()}</p>
    {#if actions}
      <div class="ui-empty-actions">{@render actions()}</div>
    {/if}
  </div>
{/if}

<style>
  .ui-empty-line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-12);
    padding: var(--space-10) var(--space-12);
    font-size: var(--font-size-sm);
    color: var(--color-text-3);
  }

  .ui-empty-block {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-12);
    width: 100%;
    max-width: 480px;
    padding: var(--space-24) var(--space-8);
  }

  .ui-empty-icon {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
  }

  .ui-empty-title {
    font-size: var(--font-size-heading);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .ui-empty-text {
    line-height: var(--line-height-message);
    color: var(--color-text-2);
  }

  .ui-empty-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
    margin-top: var(--space-4);
  }
</style>
