<script lang="ts">
  import { currentMoment } from "../lib/current-day.svelte";
  import { dayLabel } from "../lib/day-label";

  let {
    label,
    date,
    episode,
  }: {
    label: string;
    date?: string;
    episode?: string;
  } = $props();

  const text = $derived(date === undefined ? label : dayLabel(date, currentMoment()));
  const named = $derived(episode === undefined ? text : `${text}, ${episode}`);
</script>

<div class="feed-divider" role="separator" aria-label={named}>
  <span>{text}</span>
  {#if episode !== undefined}
    <span class="feed-divider-episode">{episode}</span>
  {/if}
</div>

<style>
  .feed-divider {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    margin: var(--space-4) 0;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);

    &::before,
    &::after {
      content: "";
      flex: 1;
      height: 1px;
      background: var(--color-line-soft);
    }
  }

  .feed-divider-episode {
    margin-left: calc(-1 * var(--space-4));
    font-family: var(--font-code);
  }
</style>
