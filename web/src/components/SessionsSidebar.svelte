<script lang="ts">
  import { tick } from "svelte";
  import { ws } from "../lib/ws.svelte";
  import { Icon } from "../lib/icons";
  import SessionRow from "./SessionRow.svelte";

  let {
    overlay,
    onClose,
    onSelect,
  }: {
    /** Shown as a drawer over the page (narrow screens) rather than a column. */
    overlay: boolean;
    onClose: () => void;
    onSelect: (runId: string) => void;
  } = $props();

  let finishedOpen = $state(false);
  let headingEl: HTMLHeadingElement | undefined = $state();

  const sessions = ws.sessions;
  let selectedRunId = $derived(sessions.view?.runId ?? null);

  // As a drawer, take focus when opened so keyboard users land inside it.
  $effect(() => {
    if (!overlay) return;
    void tick().then(() => headingEl?.focus());
  });
</script>

<aside
  id="sessions-sidebar"
  class="sessions-sidebar"
  class:overlay
  aria-labelledby="sessions-sidebar-title"
>
  <div class="sessions-head">
    <h2 id="sessions-sidebar-title" class="sessions-title" tabindex="-1" bind:this={headingEl}>
      Sessions
    </h2>
    {#if sessions.live.length > 0}
      <span class="sessions-live-count">{sessions.live.length} live</span>
    {/if}
    <button
      type="button"
      class="sessions-close"
      onclick={onClose}
      aria-label="Hide sessions"
      title="Hide sessions"
    >
      <Icon name="close" size={14} />
    </button>
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
      {#if sessions.live.length > 0}
        <ul class="sessions-list" aria-label="Live sessions">
          {#each sessions.live as session (session.run_id)}
            <SessionRow {session} selected={session.run_id === selectedRunId} {onSelect} />
          {/each}
        </ul>
      {:else}
        <p class="sessions-empty">
          Nothing is running. Work your agent hands off, scheduled pulses, and conversations with
          other people show up here while they run.
        </p>
      {/if}

      <div class="sessions-finished">
        <button
          type="button"
          class="sessions-disclosure"
          aria-expanded={finishedOpen}
          aria-controls="sessions-finished-list"
          onclick={() => (finishedOpen = !finishedOpen)}
        >
          <span class="sessions-disclosure-chevron" class:open={finishedOpen}>
            <Icon name="chevron" size={12} />
          </span>
          Finished
          {#if sessions.completed.length > 0}
            <span class="sessions-disclosure-count">
              {sessions.completed.length}{sessions.nextCursor ? "+" : ""}
            </span>
          {/if}
        </button>
        {#if finishedOpen}
          <div id="sessions-finished-list">
            {#if sessions.completed.length > 0}
              <ul class="sessions-list" aria-label="Finished sessions">
                {#each sessions.completed as session (session.run_id)}
                  <SessionRow {session} selected={session.run_id === selectedRunId} {onSelect} />
                {/each}
              </ul>
            {:else}
              <p class="sessions-empty">Nothing has finished yet.</p>
            {/if}
            {#if sessions.nextCursor}
              <button
                type="button"
                class="sessions-text-btn sessions-more"
                disabled={sessions.loadingMore}
                onclick={() => void sessions.loadMore()}
              >
                {sessions.loadingMore ? "Loading…" : "Show older"}
              </button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</aside>
