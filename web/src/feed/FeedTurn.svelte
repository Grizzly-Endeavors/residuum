<script lang="ts">
  import { untrack } from "svelte";
  import type { ObservedTurn } from "../lib/observed-turns.svelte";
  import { turnEndingLine } from "./activity";
  import ActivityLine from "./ActivityLine.svelte";
  import FeedItemView from "./FeedItemView.svelte";
  import TurnHead from "./TurnHead.svelte";
  import { drawnParts, gapsWithin, type FeedTurn } from "./turns";

  interface Props {
    turn: FeedTurn;
    agent: string;
    observed?: ObservedTurn;
    onStop?: () => void;
  }

  let { turn, agent, observed, onStop }: Props = $props();

  const gaps = $derived(observed?.gaps ?? []);
  const worked = $derived(turn.parts.some((part) => part.kind === "activity"));
  const parts = $derived(drawnParts(turn, gaps));
  const lastActivity = $derived(parts.findLastIndex((part) => part.kind === "activity"));
  const failed = $derived(
    turn.parts.some((part) => part.kind === "message" && part.item.kind === "turn-failure"),
  );
  const ending = $derived(turn.live ? null : turnEndingLine(observed, worked && !failed));

  let turnEl = $state<HTMLDivElement>();
  let refocus = false;
  $effect.pre(() => {
    if (turn.live) return;
    untrack(() => {
      refocus =
        turnEl?.querySelector("[data-turn-head]")?.contains(document.activeElement) ?? false;
    });
  });
  $effect(() => {
    if (turn.live) return;
    untrack(() => {
      if (!refocus) return;
      refocus = false;
      const summaries = turnEl?.querySelectorAll<HTMLElement>("[data-activity-summary]");
      summaries?.item(summaries.length - 1).focus();
    });
  });
</script>

<div class="feed-turn" data-feed-turn bind:this={turnEl}>
  {#each parts as part, index (part.key)}
    {#if part.kind === "activity"}
      <ActivityLine
        {agent}
        steps={part.steps}
        live={turn.live && index === parts.length - 1}
        gaps={gapsWithin(gaps, part, { first: index === 0, last: index === lastActivity })}
      />
    {:else}
      <div class="feed-item" data-feed-item data-kind={part.item.kind}>
        <FeedItemView item={part.item} {agent} />
      </div>
    {/if}
  {/each}
  {#if turn.live}
    <TurnHead {observed} {onStop} />
  {:else if ending !== null}
    <p class="turn-ending">{ending}</p>
  {/if}
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

  /* Messages the agent sent one after another stand apart by more than the
     paragraphs inside one do. */
  .feed-item + .feed-item {
    margin-top: var(--space-12);
  }

  .turn-ending {
    display: flex;
    align-items: center;
    min-height: 28px;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
    font-variant-numeric: tabular-nums;
  }
</style>
