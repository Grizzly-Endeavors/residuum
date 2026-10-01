<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { failureLine } from "../../lib/agent-failure";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import { formatLocation, locationAt, type Place } from "../../lib/routes";
  import { scheduled } from "../../lib/scheduled.svelte";
  import type { ScheduledCurrentRun } from "../../lib/types";
  import { Badge, Banner, Button, EmptyState, IconButton, Skeleton, Toggle } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import { agentPlaceLabel } from "../../shell/rail-model";
  import { followLink } from "../home/follow-link";
  import { runTitle } from "../home/home-model";
  import {
    actionWhen,
    lastRun,
    notRunningTitle,
    overlapNote,
    pulseCadence,
    pulseIcon,
    pulseNext,
    pulsesHeld,
  } from "./schedule-model";

  // An agent's pulses and scheduled actions: when each runs next and how it
  // last went, a switch to pause a pulse, and Cancel for an action. The agent
  // sets these up itself, so changing one means asking it in chat. Only a
  // running agent answers for its schedule, so a stopped one offers Start.

  let { agent }: { agent: string } = $props();

  const uid = $props.id();
  const summary = $derived(hub.agent(agent));
  /** `null` until the hub's agent list names this agent. */
  const agentState = $derived(hub.displayStateOf(agent));
  const live = $derived(agentState === "running" || agentState === "stopping");
  const chat = $derived<Place>({ kind: "chat", agent });

  let now = $state(Date.now());
  $effect(() => {
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 30_000);
    return () => window.clearInterval(timer);
  });

  onMount(() => {
    scheduled.startWatching({ onFrame: (listener) => ws.onFrame(listener), watches: ws.watches });
  });
  onDestroy(() => {
    scheduled.stopWatching();
  });

  // Load on arriving at a running agent, and again when a stopped one starts.
  $effect(() => {
    if (live) untrack(() => void scheduled.load());
  });

  let starting = $state(false);
  async function start(): Promise<void> {
    starting = true;
    await hub.startAgent(agent);
    starting = false;
  }
</script>

{#snippet running(run: ScheduledCurrentRun)}
  {@const note = overlapNote(run, now)}
  <Badge tone="accent" dot>Running</Badge>
  {#if note}<span class="overlap">{note}</span>{/if}
{/snippet}

<PlaceHeader title={agent} {agent} sub={agentPlaceLabel("schedule")}>
  {#if live}
    <span class="reload">
      <IconButton
        icon="reload"
        label="Reload the schedule"
        size="sm"
        loading={scheduled.loading}
        onclick={() => void scheduled.load()}
      />
    </span>
  {/if}
</PlaceHeader>

<div class="schedule-scroll">
  <div class="schedule">
    {#if agentState === "stopped" || agentState === "failed" || agentState === "starting"}
      <EmptyState
        variant="block"
        icon={agentState === "failed" ? "warning" : "pause"}
        title={notRunningTitle(agent, agentState)}
        headingLevel={2}
      >
        {#if agentState === "failed"}{failureLine(summary?.last_error?.kind)}{/if}
        Its pulses and scheduled actions show here while it runs.
        {#snippet actions()}
          <Button
            variant="primary"
            icon="play"
            loading={starting || agentState === "starting"}
            onclick={() => void start()}>Start {agent}</Button
          >
        {/snippet}
      </EmptyState>
    {:else}
      {#if scheduled.loadError}
        <Banner tone="error">
          {scheduled.loadError}
          {#snippet actions()}
            <Button size="sm" onclick={() => void scheduled.load()}>Try again</Button>
          {/snippet}
        </Banner>
      {/if}

      {#if !scheduled.loaded}
        {#if !scheduled.loadError}
          <Skeleton lines={4} label="Loading the schedule" />
        {/if}
      {:else}
        <section aria-labelledby="{uid}-pulses">
          <h2 class="heading" id="{uid}-pulses">Pulses</h2>
          {#if pulsesHeld(scheduled.pulses)}
            <Banner tone="warn">
              None of these pulses will run. Pulses are turned off in {agent}'s settings, or its
              settings can't be read.
              {#snippet actions()}
                <Button
                  size="sm"
                  onclick={() => void router.openSettings({ scope: agent, section: "schedule" })}
                  >Open settings</Button
                >
              {/snippet}
            </Banner>
          {/if}
          {#if scheduled.pulses.length === 0}
            <EmptyState>
              No pulses yet. {agent} sets one up for a check it should repeat, like looking through your
              inbox every morning.
            </EmptyState>
          {:else}
            <ul class="rows">
              {#each scheduled.pulses as pulse (pulse.name)}
                {@const title = runTitle(pulse.name)}
                {@const cadence = pulseCadence(pulse)}
                <li class="row" data-problem={pulse.problems.length > 0 || undefined}>
                  <span class="row-icon"><Icon name={pulseIcon(pulse)} size={15} /></span>
                  <div class="row-text">
                    <p class="row-title">
                      {title}
                      {#if pulse.current_run}{@render running(pulse.current_run)}{/if}
                    </p>
                    {#if cadence || pulse.agent}
                      <p class="row-sub">
                        {#if cadence}<span>{cadence}</span>{/if}
                        {#if pulse.agent}<span>Skill: {pulse.agent}</span>{/if}
                      </p>
                    {/if}
                    {#if pulse.last_outcome}
                      <p
                        class="row-sub"
                        data-failed={pulse.last_outcome.status === "failed" || undefined}
                      >
                        {lastRun(pulse.last_outcome, now)}
                      </p>
                    {/if}
                    {#each pulse.problems as problem (problem)}
                      <p class="problem">{problem}</p>
                    {/each}
                  </div>
                  <div class="row-end">
                    <span class="when">{pulseNext(pulse, agentState ?? "running", now)}</span>
                    <Toggle
                      label={title}
                      labelHidden
                      layout="inline"
                      checked={pulse.enabled}
                      loading={scheduled.pending.has(pulse.name)}
                      disabled={pulse.schedule === null}
                      onchange={() => void scheduled.toggleEnabled(pulse)}
                    />
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section aria-labelledby="{uid}-actions">
          <h2 class="heading" id="{uid}-actions">Scheduled actions</h2>
          {#if scheduled.actions.length === 0}
            <EmptyState>
              Nothing scheduled. {agent} schedules a one-off action when you ask it to do something later.
            </EmptyState>
          {:else}
            <ul class="rows">
              {#each scheduled.actions as action (action.id)}
                {@const title = runTitle(action.name)}
                <li class="row">
                  <span class="row-icon"><Icon name="clock" size={15} /></span>
                  <div class="row-text">
                    <p class="row-title">
                      {title}
                      {#if action.current_run}{@render running(action.current_run)}{/if}
                    </p>
                    {#if action.agent}<p class="row-sub">Skill: {action.agent}</p>{/if}
                  </div>
                  <div class="row-end">
                    <time class="when" datetime={action.run_at}
                      >{actionWhen(action, agentState ?? "running", now)}</time
                    >
                    <Button
                      variant="quiet"
                      size="sm"
                      aria-label="Cancel {title}"
                      loading={scheduled.pending.has(action.id)}
                      onclick={() => void scheduled.cancelAction(action)}>Cancel</Button
                    >
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <p class="note">
          {agent} sets these up as it learns what you need. To add or change one,
          <a href={formatLocation(locationAt(chat))} onclick={(event) => followLink(event, chat)}
            >ask {agent} in chat</a
          >.
        </p>
      {/if}
    {/if}
  </div>
</div>

<style>
  .reload {
    align-self: center;
    margin-left: auto;
  }

  .schedule-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    container-type: inline-size;
  }

  .schedule {
    display: flex;
    flex-direction: column;
    gap: var(--space-32);
    max-width: calc(var(--layout-reading-width) + 2 * var(--space-32));
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
  }

  .heading {
    margin-bottom: var(--space-10);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-10);
    list-style: none;
  }

  .row {
    display: grid;
    grid-template-columns: 30px minmax(0, 1fr) auto;
    align-items: start;
    gap: var(--space-12);
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-1);
    }
  }

  .row-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
  }

  .row[data-problem] .row-icon {
    background: var(--color-err-tint);
    color: var(--color-err-text);
  }

  .row-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .row-title {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-8);
    color: var(--color-text);
  }

  .row-sub {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4) var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);

    &[data-failed] {
      color: var(--color-err-text);
    }
  }

  .overlap,
  .problem {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .row-end {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    min-height: 30px;
  }

  .when {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .note {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  /* Narrow: the next run and the control go under the text. */
  @container (max-width: 520px) {
    .row {
      grid-template-columns: 30px minmax(0, 1fr);
      row-gap: var(--space-6);
    }

    .row-end {
      grid-column: 2;
      justify-content: space-between;
    }
  }
</style>
