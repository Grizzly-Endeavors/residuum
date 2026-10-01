<script lang="ts">
  import { ws } from "../lib/ws.svelte";
  import { Icon } from "../lib/icons";
  import {
    SESSION_CATEGORIES,
    categoryDescription,
    categoryHeading,
    categoryIdleText,
    groupByCategory,
  } from "../lib/session-format";
  import type { SessionCategory } from "../lib/types";
  import SessionRow from "./SessionRow.svelte";
  import OutboundTaskRow from "./OutboundTaskRow.svelte";

  let {
    onSelect,
  }: {
    onSelect: (runId: string) => void;
  } = $props();

  /** Groups the user has collapsed; every group starts open. */
  let collapsed = $state<Record<SessionCategory, boolean>>({
    external: false,
    scheduled: false,
    spawned: false,
    artifact: false,
  });
  /** Groups whose finished runs are shown; every group starts closed. */
  let finishedOpen = $state<Record<SessionCategory, boolean>>({
    external: false,
    scheduled: false,
    spawned: false,
    artifact: false,
  });

  const sessions = $derived(ws.sessions);
  let selectedRunId = $derived(sessions.view?.runId ?? null);

  let liveByCategory = $derived(groupByCategory(sessions.live));
</script>

<section id="sessions-sidebar" class="sessions-sidebar" aria-labelledby="sessions-sidebar-title">
  <div class="sessions-head">
    <h2 id="sessions-sidebar-title" class="sessions-title">Sessions</h2>
    {#if sessions.live.length + sessions.outbound.length > 0}
      <span class="sessions-live-count">{sessions.live.length + sessions.outbound.length} live</span
      >
    {/if}
  </div>

  <div class="sessions-scroll">
    {#if sessions.listError}
      <div class="sessions-error" role="alert">
        <p>{sessions.listError}</p>
        <button type="button" class="sessions-text-btn" onclick={() => void sessions.refresh()}>
          Try again
        </button>
      </div>
    {/if}

    {#if !sessions.loaded && !sessions.listError}
      <p class="sessions-empty">Loading sessions…</p>
    {:else if sessions.loaded}
      {#each SESSION_CATEGORIES as category (category)}
        {@const live = liveByCategory[category]}
        {@const outbound = category === "external" ? sessions.outbound : []}
        {@const liveCount = live.length + outbound.length}
        {@const finished = sessions.completed[category]}
        <section class="sessions-group" aria-labelledby="sessions-group-{category}-heading">
          <h3 class="sessions-group-heading" id="sessions-group-{category}-heading">
            <button
              type="button"
              class="sessions-disclosure sessions-group-toggle"
              aria-expanded={!collapsed[category]}
              aria-controls="sessions-group-{category}"
              title={categoryDescription(category)}
              onclick={() => (collapsed[category] = !collapsed[category])}
            >
              <span class="sessions-disclosure-chevron" class:open={!collapsed[category]}>
                <Icon name="chevron-down" size={12} />
              </span>
              {categoryHeading(category)}
              {#if liveCount > 0}
                <span class="sessions-group-live-count">{liveCount} live</span>
              {/if}
            </button>
          </h3>
          {#if !collapsed[category]}
            <div id="sessions-group-{category}" class="sessions-group-body">
              {#if category === "external" && sessions.outboundError}
                <div class="sessions-error" role="alert">
                  <p>{sessions.outboundError}</p>
                  <button
                    type="button"
                    class="sessions-text-btn"
                    onclick={() => void sessions.refreshOutbound()}
                  >
                    Try again
                  </button>
                </div>
              {/if}
              {#if outbound.length > 0}
                <ul class="sessions-list" aria-label="Tasks sent to other agents">
                  {#each outbound as task (task.task_id)}
                    <OutboundTaskRow {task} />
                  {/each}
                </ul>
              {/if}
              {#if live.length > 0}
                <ul class="sessions-list" aria-label="Live {category} sessions">
                  {#each live as session (session.run_id)}
                    <SessionRow {session} selected={session.run_id === selectedRunId} {onSelect} />
                  {/each}
                </ul>
              {:else if outbound.length === 0}
                <p class="sessions-empty">{categoryIdleText(category)}</p>
              {/if}

              <div class="sessions-finished">
                <button
                  type="button"
                  class="sessions-disclosure sessions-finished-toggle"
                  aria-expanded={finishedOpen[category]}
                  aria-controls="sessions-finished-{category}"
                  onclick={() => (finishedOpen[category] = !finishedOpen[category])}
                >
                  <span class="sessions-disclosure-chevron" class:open={finishedOpen[category]}>
                    <Icon name="chevron-down" size={12} />
                  </span>
                  Finished
                  {#if finished.runs.length > 0}
                    <span class="sessions-disclosure-count">
                      {finished.runs.length}{finished.nextCursor ? "+" : ""}
                    </span>
                  {/if}
                </button>
                {#if finishedOpen[category]}
                  <div id="sessions-finished-{category}">
                    {#if finished.runs.length > 0}
                      <ul class="sessions-list" aria-label="Finished {category} sessions">
                        {#each finished.runs as session (session.run_id)}
                          <SessionRow
                            {session}
                            selected={session.run_id === selectedRunId}
                            {onSelect}
                          />
                        {/each}
                      </ul>
                    {:else}
                      <p class="sessions-empty">Nothing has finished yet.</p>
                    {/if}
                    {#if finished.nextCursor}
                      <button
                        type="button"
                        class="sessions-text-btn sessions-more"
                        disabled={finished.loadingMore}
                        onclick={() => void finished.loadMore()}
                      >
                        {finished.loadingMore ? "Loading…" : "Show older"}
                      </button>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        </section>
      {/each}
    {/if}
  </div>
</section>
