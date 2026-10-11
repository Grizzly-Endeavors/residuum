<script lang="ts">
  import { Icon } from "../lib/icons";
  import type { ThinkingFeedItem } from "../lib/types";
  import { thoughtLabel } from "./activity";

  // The agent's reasoning as a step of an activity line. While it streams in
  // it is muted and clipped to its last few lines; once it is done it is one
  // line, "Thought for 6s", which opens to the whole of it. When it is all a
  // line holds, it is the whole of it, since the line above already says so.

  let { item, bare = false }: { item: ThinkingFeedItem; bare?: boolean } = $props();

  const uid = $props.id();
  let open = $state(false);
  const label = $derived(thoughtLabel(item));
</script>

<li class="thought" data-streaming={item.streaming ? "" : undefined}>
  {#if item.streaming}
    <div class="thought-row">
      <span class="thought-icon"><Icon name="spark" size={14} /></span>
      <span class="thought-label">Thinking</span>
    </div>
    <p class="thought-live">{item.content}</p>
  {:else if bare}
    <div class="thought-text thought-bare">{item.content}</div>
  {:else}
    <div class="thought-row">
      <span class="thought-icon"><Icon name="spark" size={14} /></span>
      <button
        type="button"
        class="thought-toggle"
        aria-expanded={open}
        aria-controls="{uid}-text"
        onclick={() => (open = !open)}>{label}</button
      >
      <span class="thought-chevron" aria-hidden="true"><Icon name="chevron-down" size={14} /></span>
    </div>
    {#if open}
      <div class="thought-text" id="{uid}-text">{item.content}</div>
    {/if}
  {/if}
</li>

<style>
  .thought {
    display: flex;
    flex-direction: column;
    min-width: 0;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  /* The row's button stretches over the whole row, as a step's does. */
  .thought-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-8);
    min-height: 28px;
    padding: var(--space-2) var(--space-6);
    border-radius: var(--corner-sm);
    transition:
      background var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:has(.thought-toggle) {
      &:hover {
        background: var(--color-stone-2);
        color: var(--color-text-2);
      }

      &:has(.thought-toggle:focus-visible) {
        outline: var(--focus-outline-width) solid var(--color-vein-bright);
        outline-offset: calc(-1 * var(--focus-outline-width));
      }
    }
  }

  .thought-icon {
    display: grid;
    flex: none;
    place-items: center;
    width: 16px;
    height: 16px;
  }

  [data-streaming] .thought-icon {
    color: var(--color-vein-bright);
  }

  .thought-toggle {
    min-width: 0;
    text-align: start;

    &::after {
      content: "";
      position: absolute;
      inset: 0;
      border-radius: inherit;
    }

    &:focus-visible {
      outline: none;
    }
  }

  .thought-chevron {
    display: grid;
    flex: none;
    margin-left: auto;
    transition: transform var(--duration-base) var(--ease-out);

    .thought-row:has([aria-expanded="true"]) & {
      transform: rotate(180deg);
    }
  }

  /* What is thought so far: its last three lines, the newest at the bottom. */
  .thought-live {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    max-height: calc(3 * var(--font-size-sm) * var(--line-height-ui));
    margin: var(--space-2) 0 var(--space-4) calc(var(--space-6) + var(--space-16) + var(--space-8));
    overflow: hidden;
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .thought-text {
    margin: var(--space-2) 0 var(--space-6) calc(var(--space-6) + var(--space-16) + var(--space-8));
    padding: var(--space-8) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .thought-bare {
    margin-left: 0;
  }

  @media (max-width: 760px) {
    .thought-row {
      min-height: var(--layout-touch-target);
    }

    .thought-live,
    .thought-text {
      margin-left: var(--space-8);
    }
  }
</style>
