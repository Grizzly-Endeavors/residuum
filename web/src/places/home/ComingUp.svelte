<script lang="ts">
  import { hub } from "../../lib/hub.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { formatLocation, locationAt, type Place } from "../../lib/routes";
  import { Skeleton } from "../../lib/ui";
  import { comingUp, runTitle, upcomingWhen } from "./home-model";
  import { followLink } from "./follow-link";

  // The soonest pulses and scheduled actions across the agents that will run
  // them. Each opens its agent's Schedule.

  let { now }: { now: number } = $props();

  const uid = $props.id();
  const runs = $derived(comingUp(hub.agents, overview.overviews));
</script>

<section aria-labelledby="{uid}-heading">
  <h2 class="home-heading" id="{uid}-heading">Coming up</h2>
  {#if !overview.loaded}
    <Skeleton lines={2} />
  {:else if runs.length === 0}
    <p class="coming-empty">Nothing scheduled.</p>
  {:else}
    <ul class="coming">
      {#each runs as { agent, run } (`${agent}:${run.kind}:${run.name}`)}
        {@const schedule = { kind: "schedule", agent } satisfies Place}
        <li class="run">
          <span class="run-text">
            <a
              class="run-title"
              href={formatLocation(locationAt(schedule))}
              onclick={(event) => followLink(event, schedule)}>{runTitle(run.name)}</a
            >
            <span class="run-sub"
              >{agent}, {run.kind === "pulse" ? "pulse" : "scheduled action"}</span
            >
          </span>
          <time class="run-when" datetime={run.at}>{upcomingWhen(run.at, now)}</time>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .coming {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .run {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: baseline;
    gap: var(--space-10);
    padding: var(--space-8) 0;
    font-size: var(--font-size-sm);
  }

  .run-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .run-title {
    color: var(--color-text);
    text-decoration: none;

    &:hover {
      text-decoration: underline;
      text-decoration-color: var(--color-line);
      text-underline-offset: 3px;
    }
  }

  .run-sub {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .run-when {
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .coming-empty {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }
</style>
