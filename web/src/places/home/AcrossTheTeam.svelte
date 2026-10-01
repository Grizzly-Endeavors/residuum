<script lang="ts">
  import { Icon } from "../../lib/icons";
  import { overview } from "../../lib/overview.svelte";
  import { formatLocation } from "../../lib/routes";
  import { Button, Skeleton } from "../../lib/ui";
  import { eventLocation, pastWhen, splitSummary } from "./home-model";
  import { followLink } from "./follow-link";

  // The newest things that happened across the team, each leading to where
  // it can be seen.

  let { now }: { now: number } = $props();

  const uid = $props.id();
  /** How many of the newest events show. */
  const SHOWN = 10;

  const events = $derived(overview.events.slice(0, SHOWN));
</script>

{#snippet summary(text: string, agent: string | null)}
  {@const parts = splitSummary(text, agent)}
  {#if parts.agent}<b>{parts.agent}</b>{/if}{parts.rest}
{/snippet}

<section aria-labelledby="{uid}-heading">
  <h2 class="home-heading" id="{uid}-heading">Across the team</h2>
  {#if overview.eventsError && !overview.eventsLoaded}
    <p class="stream-problem" role="alert">
      {overview.eventsError}
      <Button variant="quiet" size="sm" onclick={() => void overview.refreshEvents()}
        >Try again</Button
      >
    </p>
  {:else if !overview.eventsLoaded}
    <Skeleton lines={3} label="Loading what happened across the team" />
  {:else if events.length === 0}
    <p class="stream-empty">Nothing has happened yet.</p>
  {:else}
    <ol class="stream">
      {#each events as entry (entry.id)}
        <li class="event" data-level={entry.level}>
          <time class="event-time" datetime={entry.at}>{pastWhen(entry.at, now)}</time>
          <span class="event-mark">
            {#if entry.level === "info"}
              <span class="event-dot"></span>
            {:else}
              <Icon name="warning" size={13} />
            {/if}
          </span>
          {#if entry.target}
            {@const location = eventLocation(entry.target)}
            <a
              class="event-text"
              href={formatLocation(location)}
              onclick={(event) => followLink(event, location)}
              >{@render summary(entry.summary, entry.agent)}</a
            >
          {:else}
            <span class="event-text">{@render summary(entry.summary, entry.agent)}</span>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .stream {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .event {
    display: grid;
    grid-template-columns: 58px 14px minmax(0, 1fr);
    align-items: start;
    gap: var(--space-8);
    padding: var(--space-6) 0;
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }

  .event-time {
    padding-top: var(--space-2);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .event-mark {
    display: grid;
    place-items: center;
    height: calc(var(--font-size-sm) * 1.5);
    color: var(--color-err-text);
  }

  .event-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-text-3);
  }

  .event-text {
    min-width: 0;
    color: var(--color-text-2);
    overflow-wrap: anywhere;
    text-decoration: none;

    & b {
      color: var(--color-text);
      font-weight: var(--font-weight-medium);
    }
  }

  a.event-text:hover {
    color: var(--color-text);
    text-decoration: underline;
    text-decoration-color: var(--color-line);
    text-underline-offset: 3px;
  }

  .event[data-level="error"] .event-text {
    color: var(--color-err-text);
  }

  .stream-empty {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .stream-problem {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }
</style>
