<script lang="ts">
  import { currentMoment } from "../lib/current-day.svelte";
  import { messageTimeLabel, parseTimestamp } from "../lib/time";

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
