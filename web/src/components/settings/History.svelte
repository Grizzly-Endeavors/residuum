<script lang="ts">
  import {
    fetchCheckpointDetail,
    fetchCheckpointDiff,
    fetchCheckpointFile,
    fetchCheckpointStats,
    fetchCheckpoints,
    restoreCheckpoint,
    undoCheckpoint,
  } from "../../lib/api";
  import {
    encryptedRestoreHint,
    formatBytes,
    isEncryptedConfigFile,
    triggerLabel,
  } from "../../lib/checkpoints";
  import { userErrorMessage } from "../../lib/errors";
  import { relativeTime } from "../../lib/time";
  import { toast } from "../../lib/toast.svelte";
  import type { CheckpointDetail, CheckpointSummary, RepoKind, RepoStats } from "../../lib/types";

  let repo = $state<RepoKind>("workspace");
  let pathFilter = $state("");
  let stats = $state<Record<RepoKind, RepoStats | null>>({ workspace: null, config: null });

  let items = $state<CheckpointSummary[]>([]);
  let nextCursor = $state<string | null>(null);
  let listLoading = $state(true);
  let listError = $state("");

  let selectedId = $state<string | null>(null);
  let detail = $state<CheckpointDetail | null>(null);
  let detailLoading = $state(false);
  let detailError = $state("");

  // Per-changed-path diff text, fetched on demand and cached by path for
  // the currently selected checkpoint.
  let diffs = $state<Record<string, string | null>>({});
  let openPath = $state<string | null>(null);
  let restoringPath = $state<string | null>(null);
  let undoing = $state(false);

  async function loadStats(): Promise<void> {
    try {
      const [workspace, config] = await Promise.all([
        fetchCheckpointStats("workspace"),
        fetchCheckpointStats("config"),
      ]);
      stats = { workspace, config };
    } catch {
      // Stats are a footnote, not load-bearing — the list still works without them.
    }
  }

  async function loadList(reset: boolean): Promise<void> {
    listLoading = true;
    listError = "";
    try {
      const page = await fetchCheckpoints({
        repo,
        path: pathFilter.trim() || undefined,
        before: reset ? undefined : (nextCursor ?? undefined),
        limit: 30,
      });
      items = reset ? page.items : [...items, ...page.items];
      nextCursor = page.next_cursor;
    } catch (err: unknown) {
      listError = userErrorMessage(err, { action: "Couldn't load checkpoint history." });
    } finally {
      listLoading = false;
    }
  }

  function switchRepo(next: RepoKind): void {
    if (repo === next) return;
    repo = next;
    selectedId = null;
    detail = null;
    diffs = {};
    void loadList(true);
  }

  function applyFilter(): void {
    void loadList(true);
  }

  async function selectCheckpoint(id: string): Promise<void> {
    selectedId = id;
    detail = null;
    detailError = "";
    diffs = {};
    openPath = null;
    detailLoading = true;
    try {
      detail = await fetchCheckpointDetail(id, repo);
    } catch (err: unknown) {
      detailError = userErrorMessage(err, { action: "Couldn't load this checkpoint." });
    } finally {
      detailLoading = false;
    }
  }

  async function toggleDiff(path: string): Promise<void> {
    if (openPath === path) {
      openPath = null;
      return;
    }
    openPath = path;
    if (path in diffs) return;
    try {
      diffs[path] = await fetchCheckpointDiff(selectedId as string, repo, path);
    } catch (err: unknown) {
      diffs[path] = `Couldn't load this diff: ${userErrorMessage(err, { action: "" })}`;
    }
  }

  async function viewFullFile(path: string): Promise<void> {
    if (!selectedId) return;
    try {
      const content = await fetchCheckpointFile(selectedId, repo, path);
      diffs[path] = content;
      openPath = path;
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't load ${path} at this checkpoint.` }));
    }
  }

  async function handleRestorePath(path: string): Promise<void> {
    if (!selectedId) return;
    restoringPath = path;
    try {
      const outcome = await restoreCheckpoint(selectedId, repo, path);
      toast.success(`Restored ${path}.`);
      void loadStats();
      if (repo === "workspace" || outcome.restored_paths.length > 0) void loadList(true);
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: `Couldn't restore ${path}.` }));
    } finally {
      restoringPath = null;
    }
  }

  async function handleUndo(): Promise<void> {
    if (!selectedId) return;
    undoing = true;
    try {
      const outcome = await undoCheckpoint(selectedId, repo);
      const parts = [`Reverted ${outcome.reverted_paths.length} path(s).`];
      if (outcome.skipped_paths.length > 0) {
        parts.push(`Skipped (changed again since): ${outcome.skipped_paths.join(", ")}.`);
      }
      toast.success(parts.join(" "));
      void loadList(true);
      void loadStats();
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't undo this checkpoint." }));
    } finally {
      undoing = false;
    }
  }

  void loadStats();
  void loadList(true);
</script>

<div class="settings-section history-section">
  <div class="settings-group">
    <div class="history-toolbar">
      <div class="settings-mode-selector">
        <button
          class="settings-mode-btn"
          class:active={repo === "workspace"}
          onclick={() => switchRepo("workspace")}
        >
          Workspace
        </button>
        <button
          class="settings-mode-btn"
          class:active={repo === "config"}
          onclick={() => switchRepo("config")}
        >
          Config
        </button>
      </div>
      <form
        class="history-filter"
        onsubmit={(e) => {
          e.preventDefault();
          applyFilter();
        }}
      >
        <input
          type="text"
          placeholder="Filter by path..."
          bind:value={pathFilter}
          class="history-filter-input"
        />
        <button type="submit" class="btn btn-secondary btn-sm">Filter</button>
      </form>
    </div>

    {#if stats[repo]}
      <p class="history-stats">
        {stats[repo]?.checkpoint_count} checkpoint(s) &middot; {formatBytes(
          stats[repo]?.on_disk_bytes ?? 0,
        )} on disk
      </p>
    {/if}

    <div class="history-layout">
      <div class="history-list">
        {#if listLoading && items.length === 0}
          <p class="empty-state">Reading history.</p>
        {:else if listError}
          <p class="empty-state history-error">{listError}</p>
        {:else if items.length === 0}
          <p class="empty-state">No checkpoints yet.</p>
        {:else}
          {#each items as cp (cp.id)}
            <button
              type="button"
              class="history-item"
              class:active={selectedId === cp.id}
              onclick={() => void selectCheckpoint(cp.id)}
            >
              <span class="history-item-time">{relativeTime(cp.timestamp)}</span>
              <span class="history-item-trigger">{triggerLabel(cp.trigger)}</span>
              <span class="history-item-address">{cp.address}</span>
              <span class="history-item-summary">{cp.summary}</span>
              <span class="history-item-count"
                >{cp.changed_path_count} path{cp.changed_path_count === 1 ? "" : "s"}</span
              >
            </button>
          {/each}
          {#if nextCursor}
            <button
              type="button"
              class="btn btn-secondary btn-sm history-load-more"
              disabled={listLoading}
              onclick={() => void loadList(false)}
            >
              {listLoading ? "Loading..." : "Load more"}
            </button>
          {/if}
        {/if}
      </div>

      <div class="history-detail">
        {#if !selectedId}
          <p class="empty-state">Select a checkpoint to see what it changed.</p>
        {:else if detailLoading}
          <p class="empty-state">Loading.</p>
        {:else if detailError}
          <p class="empty-state history-error">{detailError}</p>
        {:else if detail}
          <div class="history-detail-header">
            <div>
              <div class="history-detail-summary">{detail.summary.summary}</div>
              <div class="history-detail-meta">
                {triggerLabel(detail.summary.trigger)} &middot; {detail.summary.address}
                {#if detail.summary.run_id}&middot; run {detail.summary.run_id}{/if}
                &middot; {relativeTime(detail.summary.timestamp)}
              </div>
            </div>
            <button
              type="button"
              class="btn btn-sm btn-secondary"
              disabled={undoing}
              onclick={() => void handleUndo()}
              title="Revert every path this checkpoint changed back to its content just before it"
            >
              {undoing ? "Undoing..." : "Undo this checkpoint"}
            </button>
          </div>

          {#if detail.changed_paths.length === 0}
            <p class="empty-state">This checkpoint changed nothing.</p>
          {:else}
            <ul class="history-paths">
              {#each detail.changed_paths as cp (cp.path)}
                {@const encrypted = repo === "config" && isEncryptedConfigFile(cp.path)}
                <li class="history-path">
                  <div class="history-path-row">
                    <span class="history-path-name">{cp.path}</span>
                    <span class="history-path-kind kind-{cp.kind}">{cp.kind}</span>
                    <div class="history-path-actions">
                      {#if !encrypted}
                        <button
                          type="button"
                          class="btn btn-secondary btn-sm"
                          onclick={() => void toggleDiff(cp.path)}
                        >
                          {openPath === cp.path ? "Hide diff" : "View diff"}
                        </button>
                        <button
                          type="button"
                          class="btn btn-secondary btn-sm"
                          onclick={() => void viewFullFile(cp.path)}
                        >
                          View file
                        </button>
                      {/if}
                      <button
                        type="button"
                        class="btn btn-primary btn-sm"
                        disabled={restoringPath === cp.path}
                        onclick={() => void handleRestorePath(cp.path)}
                      >
                        {restoringPath === cp.path ? "Restoring..." : "Restore"}
                      </button>
                    </div>
                  </div>
                  {#if encrypted}
                    <p class="history-encrypted-hint">{encryptedRestoreHint(cp.path)}</p>
                  {:else if openPath === cp.path}
                    <pre class="history-diff">{diffs[cp.path] ?? "Loading..."}</pre>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .history-section {
    max-width: 100%;
  }

  .history-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-3);
    margin-bottom: var(--s-2);
  }

  .history-filter {
    display: flex;
    gap: var(--s-2);
  }

  .history-filter-input {
    min-width: 200px;
  }

  .history-stats {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    margin-bottom: var(--s-3);
  }

  .history-layout {
    display: flex;
    gap: var(--s-4);
    min-height: 0;
  }

  .history-list {
    width: 340px;
    flex-shrink: 0;
    max-height: 60vh;
    overflow-y: auto;
  }

  .history-item {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 var(--s-2);
    width: 100%;
    padding: var(--s-2) var(--s-3);
    margin-bottom: 4px;
    background: var(--bg-deep);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    text-align: left;
    cursor: pointer;
    color: var(--text-muted);
  }

  .history-item:hover {
    border-color: var(--border);
  }

  .history-item.active {
    background: var(--vein-glow);
    border-color: var(--vein-dim);
    color: var(--text);
  }

  .history-item-time {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .history-item-trigger {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--vein);
  }

  .history-item-address {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--moss-hover);
  }

  .history-item-summary {
    flex-basis: 100%;
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .history-item-count {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .history-load-more {
    width: 100%;
    margin-top: var(--s-2);
  }

  .history-detail {
    flex: 1;
    min-width: 0;
  }

  .history-detail-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--s-3);
    margin-bottom: var(--s-3);
    padding-bottom: var(--s-3);
    border-bottom: 1px solid var(--border-subtle);
  }

  .history-detail-summary {
    font-size: var(--fs-base);
    color: var(--text);
  }

  .history-detail-meta {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    margin-top: 2px;
  }

  .history-paths {
    list-style: none;
  }

  .history-path {
    padding: var(--s-2) 0;
    border-bottom: 1px solid var(--border-subtle);
  }

  .history-path-row {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }

  .history-path-name {
    flex: 1;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .history-path-kind {
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    padding: 1px 6px;
    border-radius: var(--radius-sm);
  }

  .history-path-kind.kind-added {
    color: var(--success);
    background: rgba(46, 204, 113, 0.08);
  }

  .history-path-kind.kind-modified {
    color: var(--vein);
    background: var(--vein-faint);
  }

  .history-path-kind.kind-deleted {
    color: var(--error);
    background: var(--error-bg);
  }

  .history-path-actions {
    display: flex;
    gap: var(--s-2);
    flex-shrink: 0;
  }

  .history-encrypted-hint {
    margin-top: var(--s-2);
    font-size: var(--fs-xs);
    color: var(--text-dim);
    font-style: italic;
  }

  .history-diff {
    margin-top: var(--s-2);
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
    max-height: 320px;
    overflow: auto;
  }

  .history-error {
    color: var(--error);
  }

  @media (max-width: 800px) {
    .history-layout {
      flex-direction: column;
    }

    .history-list {
      width: 100%;
      max-height: 240px;
    }
  }
</style>
