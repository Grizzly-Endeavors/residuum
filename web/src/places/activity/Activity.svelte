<script lang="ts">
  import { failureLine } from "../../lib/agent-failure";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { overview } from "../../lib/overview.svelte";
  import { router } from "../../lib/router.svelte";
  import {
    KIND_NAMES,
    SESSION_CATEGORIES,
    finishedOutcome,
    formatLocalDateTime,
    isStoppableState,
    outboundDuration,
    outboundStatus,
    runIcon,
    runKind,
    runStatus,
  } from "../../lib/session-format";
  import type { FinishedKind } from "../../lib/sessions.svelte";
  import type { OutboundA2aTaskSummary, SessionSummary } from "../../lib/types";
  import { Badge, Banner, Button, EmptyState, SelectField, Skeleton } from "../../lib/ui";
  import type { Choice } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import { agentPlaceLabel } from "../../shell/rail-model";
  import { notRunningTitle } from "../schedule/schedule-model";
  import RunStatus from "./RunStatus.svelte";

  // What an agent is doing and has done: its live runs and the tasks it sent
  // to other agents, each with Stop, then its finished runs, paged and
  // filtered by kind. A run opens in the context panel. The agent is the
  // bound one, so the lists are the coordinator's sessions store.

  let { agent }: { agent: string } = $props();
  const label = $derived(hub.shownName(agent));

  const uid = $props.id();
  const sessions = $derived(ws.sessions);
  const summary = $derived(hub.agent(agent));
  const agentState = $derived(hub.displayStateOf(agent));
  const running = $derived(agentState === "running" || agentState === "stopping");
  const finished = $derived(sessions.finished[sessions.finishedKind]);
  const shownRun = $derived(router.panel?.kind === "session" ? router.panel.runId : null);
  const liveCount = $derived(sessions.live.length + sessions.outbound.length);

  const KINDS: readonly Choice<FinishedKind>[] = [
    { value: "all", label: "Every kind" },
    ...SESSION_CATEGORIES.map((category) => ({ value: category, label: KIND_NAMES[category] })),
  ];

  let now = $state(Date.now());
  $effect(() => {
    if (liveCount === 0) return;
    now = Date.now();
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });

  let starting = $state(false);
  async function start(): Promise<void> {
    starting = true;
    await hub.startAgent(agent);
    starting = false;
  }

  function open(run: SessionSummary): void {
    void router.openPanel({ kind: "session", agent, runId: run.run_id });
  }

  /** Which of a task's two commands is in flight, by task id. */
  let taskPending = $state<Record<string, "stop" | "unwatch">>({});

  async function settleTask(
    task: OutboundA2aTaskSummary,
    which: "stop" | "unwatch",
  ): Promise<void> {
    taskPending[task.task_id] = which;
    const after =
      which === "stop"
        ? await overview.stopTask(agent, task.task_id)
        : await overview.stopWatching(agent, task.task_id);
    delete taskPending[task.task_id];
    if (after !== null) sessions.applyOutbound(after);
    else if (overview.taskNotes[`${agent}:${task.task_id}`] === undefined) {
      void sessions.refreshOutbound();
    }
  }
</script>

{#snippet title(run: SessionSummary)}
  <span class="row-title">{run.purpose || run.address}</span>
{/snippet}

<PlaceHeader title={label} {agent} sub={agentPlaceLabel("activity")} />

<div class="activity-scroll">
  <div class="activity">
    {#if agentState === "stopped" || agentState === "failed" || agentState === "starting"}
      <EmptyState
        variant="block"
        icon={agentState === "failed" ? "warning" : "pause"}
        title={notRunningTitle(label, agentState)}
        headingLevel={2}
      >
        {#if agentState === "failed"}{failureLine(summary?.last_error?.kind)}{/if}
        What it's doing, and what it has done, shows here while it runs.
        {#snippet actions()}
          <Button
            variant="primary"
            icon="play"
            loading={starting || agentState === "starting"}
            onclick={() => void start()}>Start {label}</Button
          >
        {/snippet}
      </EmptyState>
    {:else}
      {#if sessions.listError}
        <Banner tone="error">
          {sessions.listError}
          {#snippet actions()}
            <Button size="sm" onclick={() => void sessions.refresh()}>Try again</Button>
          {/snippet}
        </Banner>
      {/if}

      {#if !sessions.loaded}
        {#if !sessions.listError}<Skeleton lines={5} label="Loading what's running" />{/if}
      {:else}
        <section aria-labelledby="{uid}-running">
          <h2 class="heading" id="{uid}-running">
            Running now <Badge count={liveCount} label="running" />
          </h2>
          {#if sessions.outboundError}
            <Banner tone="error">
              {sessions.outboundError}
              {#snippet actions()}
                <Button size="sm" onclick={() => void sessions.refreshOutbound()}>Try again</Button>
              {/snippet}
            </Banner>
          {/if}
          {#if liveCount === 0}
            <EmptyState>Nothing running. Work {label} starts on its own shows up here.</EmptyState>
          {:else}
            <ul class="rows">
              {#each sessions.live as run (run.run_id)}
                {@const error = sessions.errors.get(run.run_id)}
                <li class="row" data-current={run.run_id === shownRun || undefined}>
                  <button
                    type="button"
                    class="row-main"
                    aria-current={run.run_id === shownRun ? "true" : undefined}
                    onclick={() => open(run)}
                  >
                    <span class="row-icon"><Icon name={runIcon(run.category)} size={15} /></span>
                    <span class="row-text">
                      {@render title(run)}
                      <span class="row-sub">
                        <span>{runKind(agent, run)}</span><span>{run.source_label}</span>
                      </span>
                      {#if error}<span class="row-error">{error}</span>{/if}
                    </span>
                    <span class="row-end"><RunStatus status={runStatus(run, now)} /></span>
                  </button>
                  {#if isStoppableState(run.state)}
                    <span class="row-actions">
                      <Button
                        variant="quiet"
                        size="sm"
                        icon="stop"
                        aria-label="Stop {run.purpose || run.address}"
                        loading={sessions.stopping.has(run.address)}
                        disabled={!running}
                        onclick={() => void sessions.stop(run.address)}>Stop</Button
                      >
                    </span>
                  {/if}
                </li>
              {/each}
              {#each sessions.outbound as task (task.task_id)}
                {@const note = overview.taskNotes[`${agent}:${task.task_id}`]}
                {@const unreachable = task.unreachable_since !== null || note !== undefined}
                {@const pending = taskPending[task.task_id]}
                <li class="row" data-problem={unreachable || undefined}>
                  <div class="row-main">
                    <span class="row-icon">
                      <Icon name={unreachable ? "warning" : "handoff"} size={15} />
                    </span>
                    <span class="row-text">
                      <span class="row-title">{task.status_text || `A task for ${task.agent}`}</span
                      >
                      <span class="row-sub">
                        <span>Sent to {task.agent}</span>
                        <span>{outboundDuration(task, now)} ago</span>
                        {#if task.unreachable_since !== null}<span>Still retrying</span>{/if}
                      </span>
                      {#if note}<span class="row-error" role="status">{note}</span>{/if}
                    </span>
                    <span class="row-end"><RunStatus status={outboundStatus(task, now)} /></span>
                  </div>
                  <span class="row-actions">
                    <Button
                      variant="quiet"
                      size="sm"
                      icon="stop"
                      aria-label="Stop the task sent to {task.agent}"
                      loading={pending === "stop"}
                      disabled={!running || pending !== undefined}
                      onclick={() => void settleTask(task, "stop")}>Stop task</Button
                    >
                    {#if unreachable}
                      <Button
                        variant="quiet"
                        size="sm"
                        aria-label="Stop watching the task sent to {task.agent}"
                        loading={pending === "unwatch"}
                        disabled={!running || pending !== undefined}
                        onclick={() => void settleTask(task, "unwatch")}>Stop watching</Button
                      >
                    {/if}
                  </span>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section aria-labelledby="{uid}-finished">
          <div class="heading-row">
            <h2 class="heading" id="{uid}-finished">
              Finished
              {#if finished.runs.length > 0}
                <span class="heading-count">
                  {finished.runs.length}{finished.nextCursor === null ? "" : "+"}
                </span>
              {/if}
            </h2>
            <div class="kind">
              <SelectField
                label="Kind of run"
                labelHidden
                options={KINDS}
                bind:value={
                  () => sessions.finishedKind,
                  (value) => {
                    const kind = KINDS.find((choice) => choice.value === value);
                    if (kind) sessions.showFinished(kind.value);
                  }
                }
              />
            </div>
          </div>
          {#if finished.error}
            <Banner tone="error">
              {finished.error}
              {#snippet actions()}
                <Button size="sm" onclick={() => void finished.loadFirst()}>Try again</Button>
              {/snippet}
            </Banner>
          {:else if !finished.loaded}
            <Skeleton lines={3} label="Loading finished runs" />
          {:else if finished.runs.length === 0}
            <EmptyState>
              {sessions.finishedKind === "all"
                ? "Nothing has finished yet."
                : "Nothing of this kind has finished yet."}
            </EmptyState>
          {:else}
            <ul class="rows">
              {#each finished.runs as run (run.run_id)}
                {@const failed = run.outcome === "failed"}
                <li class="row" data-current={run.run_id === shownRun || undefined}>
                  <button
                    type="button"
                    class="row-main"
                    aria-current={run.run_id === shownRun ? "true" : undefined}
                    onclick={() => open(run)}
                  >
                    <span class="row-icon" data-failed={failed || undefined}>
                      <Icon name={failed ? "warning" : runIcon(run.category)} size={15} />
                    </span>
                    <span class="row-text">
                      {@render title(run)}
                      <span class="row-sub">
                        <span class:failed>{finishedOutcome(run)}</span>
                        <span>{runKind(agent, run)}</span>
                      </span>
                    </span>
                    <time class="row-end row-when" datetime={run.started_at}
                      >{formatLocalDateTime(run.started_at)}</time
                    >
                  </button>
                </li>
              {/each}
            </ul>
            {#if finished.nextCursor !== null}
              <div class="more">
                <Button
                  variant="quiet"
                  size="sm"
                  loading={finished.loadingMore}
                  onclick={() => void finished.loadMore()}>Show older</Button
                >
              </div>
            {/if}
          {/if}
        </section>
      {/if}
    {/if}
  </div>
</div>

<style>
  .activity-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    container-type: inline-size;
  }

  .activity {
    display: flex;
    flex-direction: column;
    gap: var(--space-32);
    max-width: calc(var(--layout-reading-width) + 2 * var(--space-32));
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
  }

  section {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
  }

  .heading-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
  }

  .heading {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .heading-count {
    color: var(--color-text-3);
    font-weight: var(--font-weight-regular);
    font-variant-numeric: tabular-nums;
  }

  .kind {
    width: 255px;
    max-width: 55%;
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    border-radius: var(--corner-md);
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-1);
    }

    /* The run open in the panel. On the tint, text is only text-2 or vein-bright. */
    &[data-current] {
      background: var(--color-vein-tint);

      & .row-title {
        color: var(--color-vein-bright);
      }

      & .row-sub,
      & .row-sub .failed,
      & .row-error,
      & .row-when,
      & :global(.run-status) {
        color: var(--color-text-2);
      }
    }
  }

  .row-main {
    display: grid;
    flex: 1;
    grid-template-columns: 30px minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-12);
    min-width: 0;
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    text-align: start;
  }

  .row-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);

    &[data-failed] {
      color: var(--color-err-text);
    }

    [data-problem] & {
      background: var(--color-err-tint);
      color: var(--color-err-text);
    }
  }

  .row-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .row-title {
    color: var(--color-text);
  }

  .row-sub {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-4) var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);

    & .failed {
      color: var(--color-err-text);
    }
  }

  .row-error {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .row-when {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .row-actions {
    display: flex;
    flex: none;
    gap: var(--space-4);
    padding-right: var(--space-8);
  }

  .more {
    display: flex;
    justify-content: center;
  }

  /* Narrow: the status goes under the text, and a task's two commands stack. */
  @container (max-width: 560px) {
    .row-main {
      grid-template-columns: 30px minmax(0, 1fr);
      row-gap: var(--space-4);
    }

    .row-end {
      grid-column: 2;
    }

    .row-actions {
      flex-direction: column;
      align-items: flex-end;
      padding-right: var(--space-4);
    }
  }
</style>
