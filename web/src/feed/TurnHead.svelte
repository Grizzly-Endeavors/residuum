<script lang="ts">
  import { formatElapsed } from "../lib/format-usage";
  import type { ObservedTurn } from "../lib/observed-turns.svelte";
  import { Button, StatusDot } from "../lib/ui";

  // The head of a running turn, below the newest thing it has made: working,
  // for how long, and Stop. It goes when the turn ends.

  interface Props {
    /** What the page saw of the turn while it ran, if it watched it. */
    observed?: ObservedTurn;
    /** Stops the turn; without it the head offers no Stop. */
    onStop?: () => void;
  }

  let { observed, onStop }: Props = $props();

  let now = $state(Date.now());
  const startedAt = $derived(observed?.startedAt ?? null);
  $effect(() => {
    if (startedAt === null) return;
    now = Date.now();
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });
  const elapsed = $derived(startedAt === null ? null : formatElapsed(now - startedAt));
</script>

<div class="turn-head" data-turn-head>
  <StatusDot state="running" working />
  <span class="turn-working">Working</span>
  {#if elapsed !== null}
    <span class="turn-time">{elapsed}</span>
  {/if}
  {#if onStop}
    <Button
      variant="secondary"
      size="sm"
      icon="stop"
      class="turn-stop"
      aria-label="Stop the reply"
      loading={observed?.stopAsked === true}
      onclick={onStop}>{observed?.stopAsked ? "Stopping…" : "Stop"}</Button
    >
  {/if}
</div>

<style>
  .turn-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    min-height: 28px;
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .turn-time {
    color: var(--color-text-3);
    font-variant-numeric: tabular-nums;
  }

  .turn-head :global(.turn-stop) {
    margin-left: var(--space-4);
  }
</style>
