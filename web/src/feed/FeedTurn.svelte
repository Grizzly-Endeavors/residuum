<script lang="ts">
  import { untrack } from "svelte";
  import type { ObservedTurn } from "../lib/observed-turns.svelte";
  import { turnEndingLine } from "./activity";
  import ActivityLine from "./ActivityLine.svelte";
  import FeedItemView from "./FeedItemView.svelte";
  import TurnHead from "./TurnHead.svelte";
  import { drawnParts, gapsWithin, turnCallCount, type FeedTurn } from "./turns";

  // One turn's output as one block, in the order it happened: runs of tool
  // calls as activity lines between what the agent said. A running turn ends
  // in its head (working, for how long, Stop) below the newest content; a
  // finished one in a line saying how it ended and how long it took, when
  // that is worth saying.

  interface Props {
    turn: FeedTurn;
    agent: string;
    /** What the page saw of this turn while it ran, if it watched it. */
    observed?: ObservedTurn;
    /** Stops the turn, while it runs. */
    onStop?: () => void;
  }

  let { turn, agent, observed, onStop }: Props = $props();

  const gaps = $derived(observed?.gaps ?? []);
  const parts = $derived(drawnParts(turn, gaps));
  /** Where the newest run of steps is, which is the only one shown open while the turn runs. */
  const lastActivity = $derived(parts.findLastIndex((part) => part.kind === "activity"));
  /** The turn ended in a failure, which says so itself and needs no "Worked for". */
  const failed = $derived(
    turn.parts.some((part) => part.kind === "message" && part.item.kind === "turn-failure"),
  );
  const ending = $derived(
    turn.live ? null : turnEndingLine(observed, turnCallCount(turn) > 0 && !failed),
  );

  // The head goes when the turn ends: focus that was on its Stop moves to the
  // newest summary, not to the page.
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

<div class="feed-turn" bind:this={turnEl}>
  {#each parts as part, index (part.key)}
    {#if part.kind === "activity"}
      <ActivityLine
        {agent}
        calls={part.calls}
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

  /* Where the head was, so a turn ending moves nothing. */
  .turn-ending {
    display: flex;
    align-items: center;
    min-height: 28px;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
    font-variant-numeric: tabular-nums;
  }
</style>
