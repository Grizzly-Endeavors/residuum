<script lang="ts">
  import { currentMoment } from "../lib/current-day.svelte";
  import { messageTimeLabel, parseTimestamp } from "../lib/time";

  // When a message was sent: the time of day, with the date before it for any
  // other day, in the reader's locale. It is quiet until its message is
  // hovered, focused or tapped (see `FeedItemView`), but always in the page,
  // so assistive technology reaches it. Reading the present moment re-words
  // "10:05" as "Oct 9, 10:05" once midnight passes.

  let { timestamp }: { timestamp: string } = $props();

  const at = $derived(parseTimestamp(timestamp));
  const label = $derived(at === null ? "" : messageTimeLabel(timestamp, currentMoment()));
</script>

{#if at !== null}
  <time
    class="message-time"
    data-message-meta
    datetime={at.toISOString()}
    title={at.toLocaleString(undefined, { dateStyle: "full", timeStyle: "medium" })}
  >
    {label}
  </time>
{/if}

<style>
  .message-time {
    flex: none;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    opacity: var(--message-meta, 1);
    transition: opacity var(--duration-fast) var(--ease-out);
  }
</style>
