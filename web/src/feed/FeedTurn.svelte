<script lang="ts">
  import type { ObservedTurn } from "../lib/observed-turns.svelte";
  import ActivityLine from "./ActivityLine.svelte";
  import FeedItemView from "./FeedItemView.svelte";
  import type { FeedTurn } from "./turns";

  // One turn's output as one block: its activity line, built from every tool
  // call of the turn, then what the agent said, in order.

  interface Props {
    turn: FeedTurn;
    agent: string;
    /** What the page saw of this turn while it ran, if it watched it. */
    observed?: ObservedTurn;
    /** Stops the turn, while it runs. */
    onStop?: () => void;
  }

  let { turn, agent, observed, onStop }: Props = $props();
</script>

<div class="feed-turn">
  <ActivityLine {agent} calls={turn.calls} live={turn.live} {observed} {onStop} />
  {#each turn.items as item (item.id)}
    <div class="feed-item" data-feed-item data-kind={item.kind}>
      <FeedItemView {item} {agent} />
    </div>
  {/each}
</div>

<style>
  .feed-turn {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    min-width: 0;
  }

  .feed-item {
    min-width: 0;
  }
</style>
