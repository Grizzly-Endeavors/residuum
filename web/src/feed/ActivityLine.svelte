<script lang="ts">
  import { untrack } from "svelte";
  import { formatElapsed } from "../lib/format-usage";
  import { Icon } from "../lib/icons";
  import type { ObservedTurn } from "../lib/observed-turns.svelte";
  import { Button, StatusDot } from "../lib/ui";
  import { activitySteps, gapNote, summarizeActivity, type StepCall } from "./activity";
  import ActivityStep from "./ActivityStep.svelte";

  // A turn's activity line, at the head of its block. While the
  // turn runs it is open: working, for how long, Stop, and each step as it
  // arrives. Once the turn ends it collapses to one summary, which opens to
  // the steps, and a step opens to its details.

  interface Props {
    /** The agent the conversation belongs to: its paths and sessions are what targets open. */
    agent: string;
    calls: StepCall[];
    /** The turn is running. */
    live: boolean;
    /** What the page saw of the turn while it ran, if it watched it. */
    observed?: ObservedTurn;
    /** Stops the turn; without it the line offers no Stop. */
    onStop?: () => void;
  }

  let { agent, calls, live, observed, onStop }: Props = $props();

  const uid = $props.id();
  const steps = $derived(activitySteps(calls));
  const gaps = $derived(observed?.gaps ?? []);
  const summary = $derived(live ? null : summarizeActivity(calls, observed));
  /** What follows the summary: how long, what failed, how it ended. */
  const meta = $derived(
    summary === null
      ? []
      : [
          { text: summary.duration, failed: false },
          { text: summary.failures, failed: true },
          { text: summary.ending, failed: false },
        ].filter((part): part is { text: string; failed: boolean } => part.text !== null),
  );
  let open = $state(false);

  let now = $state(Date.now());
  const startedAt = $derived(observed?.startedAt ?? null);
  $effect(() => {
    if (!live || startedAt === null) return;
    now = Date.now();
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });
  const elapsed = $derived(startedAt === null ? null : formatElapsed(now - startedAt));

  // Ending the turn takes the live line, and Stop with it, away: focus that
  // was on it moves to the summary that replaces it, not to the page.
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

  /** The gap notes that go before step `index`. */
  function gapsAt(index: number, last: boolean): number[] {
    return gaps.filter((at) => at === index || (last && at > index));
  }
</script>

{#snippet stepList(id?: string)}
  <ol class="activity-steps" {id}>
    {#each steps as step, index (step.id)}
      {#each gapsAt(index, false) as at, n (n)}
        <li class="activity-gap">{gapNote(at)}</li>
      {/each}
      <ActivityStep {step} {agent} />
    {/each}
    {#each gapsAt(steps.length, true) as at, n (n)}
      <li class="activity-gap">{gapNote(at)}</li>
    {/each}
  </ol>
{/snippet}

{#if live}
  <div class="activity" data-live bind:this={liveEl}>
    <div class="activity-head">
      <StatusDot state="running" working />
      <span class="activity-working">Working</span>
      {#if elapsed !== null}
        <span class="activity-time">{elapsed}</span>
      {/if}
      {#if onStop}
        <Button
          variant="secondary"
          size="sm"
          icon="stop"
          class="activity-stop"
          aria-label="Stop the reply"
          loading={observed?.stopAsked === true}
          onclick={onStop}>{observed?.stopAsked ? "Stopping…" : "Stop"}</Button
        >
      {/if}
    </div>
    {#if steps.length > 0 || gaps.length > 0}
      {@render stepList()}
    {/if}
  </div>
{:else if summary}
  <div class="activity">
    <button
      type="button"
      class="activity-summary"
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

  .activity-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    min-height: 28px;
    color: var(--color-text-2);
  }

  .activity-time {
    color: var(--color-text-3);
    font-variant-numeric: tabular-nums;
  }

  .activity-head :global(.activity-stop) {
    margin-left: var(--space-4);
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
