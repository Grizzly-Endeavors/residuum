<script lang="ts">
  import { untrack } from "svelte";
  import { fetchCheckpoints, fetchCheckpointStats } from "../../lib/api";
  import { formatBytes, historyRepos, REPO_LABELS, triggerLabel } from "../../lib/checkpoints";
  import { userErrorMessage } from "../../lib/errors";
  import { Icon } from "../../lib/icons";
  import { relativeTime } from "../../lib/time";
  import type { CheckpointSummary, RepoKind, RepoStats } from "../../lib/types";
  import { Banner, Button, EmptyState, SegmentedControl, Skeleton, TextField } from "../../lib/ui";
  import CheckpointChanges from "./CheckpointChanges.svelte";

  // The checkpoint browser behind Settings → History, for either scope: an
  // agent's workspace and config repositories, or with `agent` null the
  // team's and the hub's. Newest first, filtered by path, paged; a row opens
  // in place to what it changed, with restore and undo. Its routes answer for
  // a stopped agent too.

  let { agent }: { agent: string | null } = $props();

  const PAGE_SIZE = 30;
  const uid = $props.id();
  const repos = $derived(historyRepos(agent));

  let repo = $state<RepoKind>(untrack(() => historyRepos(agent)[0] ?? "workspace"));
  let filterText = $state("");
  let filter = $state("");
  let items = $state.raw<CheckpointSummary[]>([]);
  let nextCursor = $state<string | null>(null);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let stats = $state.raw<RepoStats | null>(null);
  let statsError = $state(false);
  let openId = $state<string | null>(null);
  /** Counts loads, so an answer for a repository or filter no longer shown is dropped. */
  let generation = 0;

  async function load(fromStart: boolean): Promise<void> {
    const mine = fromStart ? ++generation : generation;
    loading = true;
    loadError = null;
    try {
      const page = await fetchCheckpoints(agent, {
        repo,
        path: filter || undefined,
        before: fromStart ? undefined : (nextCursor ?? undefined),
        limit: PAGE_SIZE,
      });
      if (mine !== generation) return;
      items = fromStart ? page.items : [...items, ...page.items];
      nextCursor = page.next_cursor;
    } catch (err) {
      if (mine === generation) {
        loadError = userErrorMessage(err, { action: "Couldn't load this history." });
      }
    } finally {
      if (mine === generation) loading = false;
    }
  }

  async function loadStats(): Promise<void> {
    const shown = repo;
    try {
      const fetched = await fetchCheckpointStats(agent, shown);
      if (shown === repo) [stats, statsError] = [fetched, false];
    } catch (err) {
      userErrorMessage(err, { action: "Couldn't read the history's size." });
      if (shown === repo) [stats, statsError] = [null, true];
    }
  }

  /** The list and its size again, after a restore or undo added a checkpoint. */
  function refresh(): void {
    void load(true);
    void loadStats();
  }

  $effect(() => {
    void repo;
    untrack(() => {
      openId = null;
      stats = null;
      refresh();
    });
  });

  function applyFilter(text: string): void {
    filterText = text;
    filter = text.trim();
    openId = null;
    void load(true);
  }

  const statsLine = $derived.by(() => {
    if (statsError) return "Couldn't read how big this history is.";
    if (stats === null) return "";
    const count = `${String(stats.checkpoint_count)} checkpoint${stats.checkpoint_count === 1 ? "" : "s"}`;
    const since = stats.oldest === null ? "" : `, the oldest from ${relativeTime(stats.oldest)}`;
    return `${count}, ${formatBytes(stats.on_disk_bytes)} on disk${since}.`;
  });
</script>

<div class="history">
  <div class="history-bar">
    <SegmentedControl
      label="History of"
      labelHidden
      bind:value={repo}
      options={repos.map((value) => ({ value, label: REPO_LABELS[value] }))}
    />
    <form
      class="history-filter"
      role="search"
      onsubmit={(event) => {
        event.preventDefault();
        applyFilter(filterText);
      }}
    >
      <TextField
        label="Only changes to"
        labelHidden
        code
        placeholder="A file or folder, like SOUL.md"
        bind:value={filterText}
      />
      <Button type="submit" variant="secondary">Filter</Button>
      {#if filter}
        <Button variant="quiet" onclick={() => applyFilter("")}>Show all</Button>
      {/if}
    </form>
  </div>
  <p class="history-stats" aria-live="polite">{statsLine}</p>

  {#if loadError}
    <Banner tone="error">
      {loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load(true)}>Try again</Button>
      {/snippet}
    </Banner>
  {/if}
  {#if loading && items.length === 0}
    <Skeleton lines={5} label="Loading the history" />
  {:else if items.length === 0 && !loadError}
    <EmptyState>
      {#if filter}
        No checkpoint changed <code>{filter}</code>.
      {:else}
        No checkpoints yet. One is recorded around every turn and before every change made here.
      {/if}
    </EmptyState>
  {:else if items.length > 0}
    <ul class="history-list" aria-label="{REPO_LABELS[repo]} checkpoints">
      {#each items as checkpoint (checkpoint.id)}
        {@const open = openId === checkpoint.id}
        {@const files = checkpoint.changed_path_count}
        <li class="checkpoint" data-open={open || undefined}>
          <button
            type="button"
            class="checkpoint-head"
            aria-expanded={open}
            aria-controls="{uid}-{checkpoint.id}"
            onclick={() => (openId = open ? null : checkpoint.id)}
          >
            <span class="checkpoint-text">
              <span class="checkpoint-summary">{checkpoint.summary}</span>
              <span class="checkpoint-meta">
                <span>{triggerLabel(checkpoint.trigger)}, by {checkpoint.address}</span>
                <span>{files} file{files === 1 ? "" : "s"}</span>
              </span>
            </span>
            <time class="checkpoint-when" datetime={checkpoint.timestamp}>
              {relativeTime(checkpoint.timestamp)}
            </time>
            <span class="checkpoint-chevron"><Icon name="chevron-down" size={14} /></span>
          </button>
          {#if open}
            <div class="checkpoint-body" id="{uid}-{checkpoint.id}">
              <CheckpointChanges {agent} {repo} {checkpoint} onchanged={refresh} />
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if nextCursor}
      <div class="history-more">
        <Button variant="secondary" {loading} onclick={() => void load(false)}>Show older</Button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .history {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .history-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
  }

  .history-filter {
    display: flex;
    flex: 1 1 280px;
    gap: var(--space-8);
    max-width: 420px;

    & > :global(:first-child) {
      flex: 1;
      min-width: 0;
    }
  }

  .history-stats {
    min-height: 1lh;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .history-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    list-style: none;
  }

  .checkpoint-head {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto 14px;
    align-items: center;
    gap: var(--space-12);
    width: 100%;
    padding: var(--space-8) var(--space-12);
    border: 0;
    border-radius: var(--corner-md);
    background: none;
    color: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-2);
    }
  }

  .checkpoint[data-open] .checkpoint-head {
    background: var(--color-vein-tint);
  }

  .checkpoint-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .checkpoint-summary {
    overflow: hidden;
    color: var(--color-text);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .checkpoint[data-open] .checkpoint-summary {
    color: var(--color-vein-bright);
  }

  .checkpoint-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0 var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .checkpoint[data-open] .checkpoint-meta {
    color: var(--color-text-2);
  }

  .checkpoint-when {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .checkpoint-chevron {
    display: grid;
    color: var(--color-text-3);
    transition: transform var(--duration-base) var(--ease-out);
  }

  .checkpoint[data-open] .checkpoint-chevron {
    transform: rotate(180deg);
  }

  .checkpoint-body {
    padding: var(--space-4) var(--space-12) var(--space-16);
  }

  .history-more {
    display: flex;
    justify-content: center;
  }
</style>
