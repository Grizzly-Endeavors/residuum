<script lang="ts">
  import ToolGroup from "../components/ToolGroup.svelte";
  import FeedItemView from "./FeedItemView.svelte";
  import type { FeedTurn } from "./turns";

  // One turn's output as one block: its tool calls first, then what the agent
  // said, in order. The tool calls are the legacy tool rows, shown only while
  // "Show tool calls" is on.

  let { turn, agent, verbose }: { turn: FeedTurn; agent: string; verbose: boolean } = $props();
</script>

<div class="feed-turn">
  {#if verbose && turn.calls.length > 0}
    <div class="feed-item" data-feed-item data-kind="tool-group">
      <div data-legacy-view>
        <ToolGroup calls={turn.calls} verbose />
      </div>
    </div>
  {/if}
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
