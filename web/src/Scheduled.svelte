<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { scheduled } from "./lib/scheduled.svelte";
  import { ws } from "./lib/ws.svelte";
  import { formatLocalDateTime } from "./lib/session-format";
  import { Icon } from "./lib/icons";

  let { onClose }: { onClose: () => void } = $props();

  onMount(() => {
    scheduled.startWatching((listener) => ws.onFrame(listener));
    void scheduled.load();
  });
  onDestroy(() => {
    scheduled.stopWatching();
  });
</script>

<div class="settings-view scheduled-view emerges">
  <div class="settings-header">
    <span class="settings-title">Scheduled</span>
    <div class="settings-header-actions">
      <button
        class="icon-btn"
        title="Reload"
        aria-label="Reload the scheduled list"
        onclick={() => scheduled.load()}
        disabled={scheduled.loading}
      >
        <Icon name="reload" size={16} />
      </button>
      <button
        class="icon-btn"
        title="Close"
        aria-label="Close the scheduled view"
        onclick={onClose}
      >
        <Icon name="close" size={16} />
      </button>
    </div>
  </div>

  <div class="scheduled-body">
    <section class="scheduled-section">
      <h3 class="settings-group-label">Pulses</h3>
      {#if scheduled.loading && !scheduled.loaded}
        <p class="scheduled-empty">Loading…</p>
      {:else if scheduled.pulses.length === 0}
        <p class="scheduled-empty">
          No pulses configured. The agent adds pulses to HEARTBEAT.yml as it sets up ambient checks.
        </p>
      {:else}
        <ul class="scheduled-list">
          {#each scheduled.pulses as pulse (pulse.name)}
            <li class="scheduled-row" class:has-problems={pulse.problems.length > 0}>
              <div class="scheduled-row-top">
                <span
                  class="toggle-switch"
                  title={pulse.enabled ? "Disable this pulse" : "Enable this pulse"}
                >
                  <input
                    type="checkbox"
                    checked={pulse.enabled}
                    disabled={scheduled.pending.has(pulse.name) || pulse.schedule === null}
                    aria-label={pulse.enabled ? `Disable ${pulse.name}` : `Enable ${pulse.name}`}
                    onchange={() => scheduled.toggleEnabled(pulse)}
                  />
                  <span class="toggle-slider"></span>
                </span>
                <span class="scheduled-name">{pulse.name}</span>
                {#if pulse.current_run}
                  <span class="scheduled-badge scheduled-badge-running">running now</span>
                  {#if pulse.current_run.overlap}
                    <span
                      class="scheduled-badge scheduled-badge-overlap"
                      title={`Started while the previous run (started ${formatLocalDateTime(
                        pulse.current_run.overlap.previous_started_at,
                      )}) was still going`}
                    >
                      overlapping previous run
                    </span>
                  {/if}
                {/if}
              </div>
              <div class="scheduled-row-meta">
                {#if pulse.schedule}<span>Every {pulse.schedule}</span>{/if}
                {#if pulse.active_hours}<span>Active {pulse.active_hours}</span>{/if}
                {#if pulse.agent}<span>Skill: {pulse.agent}</span>{/if}
                {#if pulse.next_fire_at}
                  <span>Next: {formatLocalDateTime(pulse.next_fire_at)}</span>
                {/if}
              </div>
              {#if pulse.last_outcome}
                <div
                  class="scheduled-outcome"
                  class:failed={pulse.last_outcome.status === "failed"}
                >
                  Last run {pulse.last_outcome.status} at {formatLocalDateTime(
                    pulse.last_outcome.at,
                  )}{#if pulse.last_outcome.error}: {pulse.last_outcome.error}{/if}
                </div>
              {/if}
              {#each pulse.problems as problem (problem)}
                <div class="scheduled-problem">
                  <Icon name="warning" size={12} />
                  <span>{problem}</span>
                </div>
              {/each}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="scheduled-section">
      <h3 class="settings-group-label">Scheduled Actions</h3>
      {#if scheduled.loading && !scheduled.loaded}
        <p class="scheduled-empty">Loading…</p>
      {:else if scheduled.actions.length === 0}
        <p class="scheduled-empty">
          Nothing scheduled. The agent schedules one-off actions with schedule_action.
        </p>
      {:else}
        <ul class="scheduled-list">
          {#each scheduled.actions as action (action.id)}
            <li class="scheduled-row">
              <div class="scheduled-row-top">
                <span class="scheduled-name">{action.name}</span>
                {#if action.current_run}
                  <span class="scheduled-badge scheduled-badge-running">running now</span>
                {/if}
                <button
                  type="button"
                  class="scheduled-cancel"
                  disabled={scheduled.pending.has(action.id)}
                  onclick={() => scheduled.cancelAction(action)}
                >
                  Cancel
                </button>
              </div>
              <div class="scheduled-row-meta">
                <span>Due {formatLocalDateTime(action.run_at)}</span>
                {#if action.agent}<span>Skill: {action.agent}</span>{/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>
</div>
