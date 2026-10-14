<script lang="ts">
  import { untrack } from "svelte";
  import { Icon } from "../lib/icons";
  import { activitySteps, callsOf, gapNote, summarizeSegment, type SegmentStep } from "./activity";
  import ActivityStep from "./ActivityStep.svelte";
  import ThoughtStep from "./ThoughtStep.svelte";

  // One run of a turn's tool calls and reasoning, at its place among what the agent said.
  // While it is the newest thing in a running turn its steps show as they
  // arrive. Once the agent has said something after it, or the turn ends, it
  // collapses to one summary, which opens to the steps, and a step opens to
  // its details.

  interface Props {
    /** The agent the conversation belongs to: its paths and sessions are what targets open. */
    agent: string;
    /** What the segment holds, in the order it happened. */
    steps: readonly SegmentStep[];
    /** The segment is the newest thing in a running turn. */
    live: boolean;
    /**
     * Where the page may have missed steps, as the number of this segment's
     * steps before each: 0 is before the first, `calls.length` after the last.
     */
    gaps?: readonly number[];
  }

  let { agent, steps, live, gaps = [] }: Props = $props();

  const uid = $props.id();
  const calls = $derived(callsOf(steps));
  const callRows = $derived(activitySteps(calls));
  /** Each step as a row: a call's number among the calls is where a gap note before it goes. */
  const rows = $derived.by(() => {
    let ordinal = 0;
    return steps.map((step) =>
      step.kind === "call"
        ? { kind: "call" as const, step: callRows[ordinal], at: ordinal++ }
        : { kind: "thought" as const, item: step.item },
    );
  });
  const summary = $derived(live ? null : summarizeSegment(steps, gaps.length > 0));
  /** What follows the summary: how long it took, what failed. */
  const meta = $derived(
    summary === null
      ? []
      : [
          { text: summary.duration, failed: false },
          { text: summary.failures, failed: true },
        ].filter((part): part is { text: string; failed: boolean } => part.text !== null),
  );
  let open = $state(false);

  // Giving way to a summary takes the live steps away: focus that was on one
  // of them moves to the summary that replaces them, not to the page.
  let liveEl = $state<HTMLDivElement>();
  let summaryEl = $state<HTMLButtonElement>();
  let refocus = false;
  $effect.pre(() => {
    if (live) return;
    untrack(() => {
      refocus = liveEl?.contains(document.activeElement) ?? false;
    });
  });
  $effect(() => {
    if (live || !summaryEl) return;
    untrack(() => {
      if (refocus) summaryEl?.focus();
      refocus = false;
    });
  });

  /** The gap notes that go before the call numbered `index`. */
  function gapsAt(index: number): number[] {
    return gaps.filter((at) => at === index);
  }
</script>

{#snippet stepList(id?: string)}
  <ol class="activity-steps" {id}>
    {#each rows as row (row.kind === "call" ? row.step?.id : `thought-${String(row.item.id)}`)}
      {#if row.kind === "call" && row.step}
        {#each gapsAt(row.at) as at, n (n)}
          <li class="activity-gap">{gapNote(at)}</li>
        {/each}
        <ActivityStep step={row.step} {agent} />
      {:else if row.kind === "thought"}
        <ThoughtStep item={row.item} bare={!live && rows.length === 1} />
      {/if}
    {/each}
    {#each gapsAt(calls.length) as at, n (n)}
      <li class="activity-gap">{gapNote(at)}</li>
    {/each}
  </ol>
{/snippet}

{#if live}
  {#if rows.length > 0 || gaps.length > 0}
    <div class="activity" data-live bind:this={liveEl}>
      {@render stepList()}
    </div>
  {/if}
{:else if summary}
  <div class="activity">
    <button
      type="button"
      class="activity-summary"
      data-activity-summary
      bind:this={summaryEl}
      aria-expanded={open}
      aria-controls="{uid}-steps"
      onclick={() => (open = !open)}
    >
      <span class="activity-chevron"><Icon name="chevron-right" size={14} /></span>
      <span class="activity-text">
        {summary.text}{#each meta as part (part.text)}<span
            class="activity-meta"
            class:failed={part.failed}>{` · ${part.text}`}</span
          >{/each}
      </span>
    </button>
    {#if open}
      {@render stepList(`${uid}-steps`)}
    {/if}
  </div>
{/if}

<style>
  .activity {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-4);
    min-width: 0;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .activity-summary {
    display: inline-flex;
    align-items: flex-start;
    gap: var(--space-6);
    min-height: 28px;
    margin-left: calc(-1 * var(--space-6));
    padding: var(--space-4) var(--space-10) var(--space-4) var(--space-6);
    border-radius: var(--corner-sm);
    color: var(--color-text-3);
    text-align: start;
    transition:
      background var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-2);
      color: var(--color-text-2);
    }
  }

  .activity-chevron {
    display: grid;
    flex: none;
    place-items: center;
    height: calc(var(--font-size-sm) * var(--line-height-ui));
    transition: transform var(--duration-base) var(--ease-out);

    [aria-expanded="true"] > & {
      transform: rotate(90deg);
    }
  }

  .activity-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .activity-meta {
    font-variant-numeric: tabular-nums;

    &.failed {
      color: var(--color-err-text);
    }
  }

  /* The steps hang off a hairline under the line's mark. */
  .activity-steps {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    align-self: stretch;
    min-width: 0;
    margin: 0 0 var(--space-4) var(--space-6);
    padding: var(--space-2) 0 var(--space-2) var(--space-8);
    border-left: 1px solid var(--color-line);
    list-style: none;
  }

  .activity-gap {
    padding: var(--space-4) var(--space-6);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  @media (max-width: 760px) {
    .activity-summary {
      min-height: var(--layout-touch-target);
      align-items: center;
    }
  }
</style>
