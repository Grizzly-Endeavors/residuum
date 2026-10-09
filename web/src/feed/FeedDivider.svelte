<script lang="ts">
  import { currentMoment } from "../lib/current-day.svelte";
  import { dayLabel } from "../lib/day-label";

  // A day's start, or above a loaded episode the day it ends on and, quietly,
  // its id. Days are named from today ("Today", "Yesterday", "Oct 6") and
  // renamed as the days pass. The feed's Jump to latest pill reads the label
  // of the divider at the top of the view.

  let {
    label,
    date,
    episode,
  }: {
    /** The text of a divider that stands for no day, such as "New run". */
    label: string;
    /** The day the divider stands for, when it does. */
    date?: string;
    /** The episode the divider introduces. */
    episode?: string;
  } = $props();

  const text = $derived(date === undefined ? label : dayLabel(date, currentMoment()));
  const named = $derived(episode === undefined ? text : `${text}, ${episode}`);
</script>

<div class="feed-divider" role="separator" aria-label={named} data-divider-label={named}>
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

  /* An id, so in the code face and set a little apart from the date. */
  .feed-divider-episode {
    margin-left: calc(-1 * var(--space-4));
    font-family: var(--font-code);
  }
</style>
