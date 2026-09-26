<script lang="ts">
  import {
    fetchCheckpointDiff,
    fetchCheckpointFile,
    fetchCheckpoints,
    restoreCheckpoint,
  } from "../lib/api";
  import { triggerLabel } from "../lib/checkpoints";
  import { userErrorMessage } from "../lib/errors";
  import { relativeTime } from "../lib/time";
  import { toast } from "../lib/toast.svelte";
  import type { CheckpointSummary } from "../lib/types";

  let {
    path,
    onClose,
    onRestored,
  }: {
    path: string;
    onClose: () => void;
    onRestored: () => void;
  } = $props();

  let checkpoints = $state<CheckpointSummary[]>([]);
  let loading = $state(true);
  let loadError = $state("");

  let selectedId = $state<string | null>(null);
  let diff = $state<string | null>(null);
  let diffLoading = $state(false);
  let diffError = $state("");
  let restoring = $state(false);

  async function load(): Promise<void> {
    loading = true;
    loadError = "";
    try {
      const page = await fetchCheckpoints({ repo: "workspace", path, limit: 100 });
      checkpoints = page.items;
      if (checkpoints.length > 0 && checkpoints[0]) void selectCheckpoint(checkpoints[0].id);
    } catch (err: unknown) {
      loadError = userErrorMessage(err, { action: `Couldn't load history for ${path}.` });
    } finally {
      loading = false;
    }
  }

  async function selectCheckpoint(id: string): Promise<void> {
    selectedId = id;
    diff = null;
    diffError = "";
    diffLoading = true;
    try {
      diff = await fetchCheckpointDiff(id, "workspace", path);
    } catch (err: unknown) {
      diffError = userErrorMessage(err, { action: "Couldn't load this version's diff." });
    } finally {
      diffLoading = false;
    }
  }

  async function viewFullContent(id: string): Promise<void> {
    try {
      const content = await fetchCheckpointFile(id, "workspace", path);
      diff = content;
      diffError = "";
    } catch (err: unknown) {
      diffError = userErrorMessage(err, { action: "Couldn't load this version's content." });
    }
  }

  async function handleRestore(id: string): Promise<void> {
    restoring = true;
    try {
      const outcome = await restoreCheckpoint(id, "workspace", path);
      toast.success(`Restored ${path} (${outcome.restored_paths.length} path(s)).`);
      onRestored();
      await load();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't restore ${path}.` }));
    } finally {
      restoring = false;
    }
  }

  void load();
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="modal-backdrop file-history-backdrop"
  onclick={(e) => {
    if (e.target === e.currentTarget) onClose();
  }}
>
  <div class="file-history-panel" role="dialog" aria-modal="true" aria-label="History for {path}">
    <div class="file-history-header">
      <div>
        <h2 class="file-history-title">History</h2>
        <p class="file-history-path">{path}</p>
      </div>
      <button
        type="button"
        class="icon-btn"
        onclick={onClose}
        aria-label="Close history"
        title="Close"
      >
        &#10005;
      </button>
    </div>

    <div class="file-history-body">
      <div class="file-history-list">
        {#if loading}
          <p class="empty-state">Reading history.</p>
        {:else if loadError}
          <p class="empty-state file-history-error">{loadError}</p>
        {:else if checkpoints.length === 0}
          <p class="empty-state">No checkpoints have touched this file yet.</p>
        {:else}
          {#each checkpoints as cp (cp.id)}
            <button
              type="button"
              class="file-history-item"
              class:active={selectedId === cp.id}
              onclick={() => void selectCheckpoint(cp.id)}
            >
              <span class="file-history-item-time">{relativeTime(cp.timestamp)}</span>
              <span class="file-history-item-trigger">{triggerLabel(cp.trigger)}</span>
              <span class="file-history-item-summary">{cp.summary}</span>
            </button>
          {/each}
        {/if}
      </div>

      <div class="file-history-content">
        {#if selectedId}
          <div class="file-history-content-actions">
            <button
              type="button"
              class="btn btn-secondary btn-sm"
              onclick={() => void viewFullContent(selectedId as string)}
            >
              View full content
            </button>
            <button
              type="button"
              class="btn btn-primary btn-sm"
              disabled={restoring}
              onclick={() => void handleRestore(selectedId as string)}
            >
              {restoring ? "Restoring..." : "Restore this version"}
            </button>
          </div>
          {#if diffLoading}
            <p class="empty-state">Loading.</p>
          {:else if diffError}
            <p class="empty-state file-history-error">{diffError}</p>
          {:else if diff === null}
            <p class="empty-state">No changes to this file at this checkpoint.</p>
          {:else}
            <pre class="file-history-diff">{diff}</pre>
          {/if}
        {:else}
          <p class="empty-state">Select a checkpoint to see what changed.</p>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .file-history-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(14, 14, 16, 0.45);
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 210;
  }

  .file-history-panel {
    display: flex;
    flex-direction: column;
    width: min(900px, calc(100vw - 32px));
    height: min(640px, calc(100vh - 64px));
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow:
      var(--elev-3),
      0 0 0 1px var(--vein-faint);
    overflow: hidden;
  }

  .file-history-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: var(--s-4) var(--s-5);
    border-bottom: 1px solid var(--border-subtle);
  }

  .file-history-title {
    font-family: var(--font-display);
    font-size: var(--fs-lg);
    font-weight: 500;
    letter-spacing: 0.06em;
  }

  .file-history-path {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-dim);
    margin-top: 2px;
  }

  .file-history-body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .file-history-list {
    width: 280px;
    flex-shrink: 0;
    overflow-y: auto;
    border-right: 1px solid var(--border-subtle);
    padding: var(--s-2);
  }

  .file-history-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: var(--s-2) var(--s-3);
    margin-bottom: 4px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    text-align: left;
    cursor: pointer;
    color: var(--text-muted);
  }

  .file-history-item:hover {
    background: var(--bg-raised);
  }

  .file-history-item.active {
    background: var(--vein-glow);
    border-color: var(--vein-dim);
    color: var(--text);
  }

  .file-history-item-time {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .file-history-item-trigger {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--vein);
  }

  .file-history-item-summary {
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-history-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    padding: var(--s-4);
    overflow-y: auto;
  }

  .file-history-content-actions {
    display: flex;
    gap: var(--s-2);
    margin-bottom: var(--s-3);
  }

  .file-history-error {
    color: var(--error);
  }

  .file-history-diff {
    flex: 1;
    margin: 0;
    padding: var(--s-3);
    background: var(--bg-deep);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: 1.6;
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-word;
    overflow: auto;
  }

  @media (max-width: 640px) {
    .file-history-body {
      flex-direction: column;
    }

    .file-history-list {
      width: 100%;
      max-height: 180px;
      border-right: none;
      border-bottom: 1px solid var(--border-subtle);
    }
  }
</style>
