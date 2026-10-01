<script lang="ts">
  import { fetchCheckpointDetail, fetchCheckpointDiff, fetchCheckpointFile } from "../../lib/api";
  import {
    CHANGE_LABELS,
    encryptedRestoreHint,
    isEncryptedConfigFile,
    undoReport,
  } from "../../lib/checkpoints";
  import { configCoordinator } from "../../lib/config-coordinator";
  import { userErrorMessage } from "../../lib/errors";
  import { toast } from "../../lib/toast.svelte";
  import type { ChangedPath, CheckpointDetail, CheckpointSummary, RepoKind } from "../../lib/types";
  import { Badge, Banner, Button, Skeleton } from "../../lib/ui";
  import CheckpointText from "../../places/files/CheckpointText.svelte";

  // What one checkpoint changed, opened in place in the History list: each
  // path with its changes or its whole content at that point, Restore for a
  // path, and Undo for the whole checkpoint. Both go through the config write
  // coordinator, so a settings form showing a restored file reads it again.

  interface Props {
    agent: string | null;
    repo: RepoKind;
    checkpoint: CheckpointSummary;
    /** A restore or undo wrote files and recorded a checkpoint of its own. */
    onchanged: () => void;
  }

  let { agent, repo, checkpoint, onchanged }: Props = $props();

  type ViewMode = "diff" | "file";
  interface PathView {
    path: string;
    mode: ViewMode;
    text: string | null;
    error: string | null;
  }

  let detail = $state.raw<CheckpointDetail | null>(null);
  let loadError = $state<string | null>(null);
  let view = $state<PathView | null>(null);
  let restoring = $state<string | null>(null);
  let undoing = $state(false);
  let report = $state<{ text: string; skipped: boolean } | null>(null);

  async function load(): Promise<void> {
    loadError = null;
    try {
      detail = await fetchCheckpointDetail(agent, checkpoint.id, repo);
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load what this checkpoint changed." });
    }
  }
  void load();

  async function show(path: string, mode: ViewMode): Promise<void> {
    if (view?.path === path && view.mode === mode && view.error === null) {
      view = null;
      return;
    }
    view = { path, mode, text: null, error: null };
    try {
      const text =
        mode === "diff"
          ? await fetchCheckpointDiff(agent, checkpoint.id, repo, path)
          : await fetchCheckpointFile(agent, checkpoint.id, repo, path);
      if (view?.path === path && view.mode === mode) view.text = text ?? "";
    } catch (err) {
      const what = mode === "diff" ? `the changes to ${path}` : path;
      const error = userErrorMessage(err, { action: `Couldn't load ${what}.` });
      if (view?.path === path && view.mode === mode) view.error = error;
    }
  }

  async function restore(change: ChangedPath): Promise<void> {
    restoring = change.path;
    try {
      await configCoordinator.restore(agent, checkpoint.id, repo, change.path);
      toast.success(`Restored ${change.path}.`);
      onchanged();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't restore ${change.path}.` }));
    } finally {
      restoring = null;
    }
  }

  async function undo(): Promise<void> {
    undoing = true;
    report = null;
    try {
      const outcome = await configCoordinator.undo(agent, checkpoint.id, repo);
      report = { text: undoReport(outcome), skipped: outcome.skipped_paths.length > 0 };
      onchanged();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: "Couldn't undo these changes." }));
    } finally {
      undoing = false;
    }
  }

  const recorded = $derived(new Date(checkpoint.timestamp).toLocaleString());
</script>

<div class="changes">
  <p class="changes-meta">
    Recorded {recorded} by {checkpoint.address}{checkpoint.run_id
      ? `, in run ${checkpoint.run_id}`
      : ""}.
  </p>

  {#if loadError}
    <Banner tone="error">
      {loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load()}>Try again</Button>
      {/snippet}
    </Banner>
  {:else if detail === null}
    <Skeleton lines={2} label="Loading what changed" />
  {:else if detail.changed_paths.length === 0}
    <p class="changes-empty">This checkpoint changed no files.</p>
  {:else}
    <ul class="paths" aria-label="Files this checkpoint changed">
      {#each detail.changed_paths as change (change.path)}
        {@const encrypted = repo === "hub" && isEncryptedConfigFile(change.path)}
        {@const shown = view?.path === change.path ? view : null}
        <li class="path">
          <div class="path-row">
            <code class="path-name">{change.path}</code>
            <Badge tone={change.kind === "deleted" ? "danger" : "neutral"}>
              {CHANGE_LABELS[change.kind]}
            </Badge>
            <span class="path-actions">
              {#if !encrypted}
                <Button
                  size="sm"
                  variant="quiet"
                  aria-pressed={shown?.mode === "diff"}
                  aria-label="Changes to {change.path}"
                  onclick={() => void show(change.path, "diff")}>Changes</Button
                >
              {/if}
              {#if change.kind !== "deleted"}
                {#if !encrypted}
                  <Button
                    size="sm"
                    variant="quiet"
                    aria-pressed={shown?.mode === "file"}
                    aria-label="{change.path} as it was then"
                    onclick={() => void show(change.path, "file")}>As it was</Button
                  >
                {/if}
                <Button
                  size="sm"
                  variant="secondary"
                  loading={restoring === change.path}
                  aria-label="Restore {change.path} as it was then"
                  onclick={() => void restore(change)}>Restore</Button
                >
              {/if}
            </span>
          </div>
          {#if encrypted}
            <p class="path-note">{encryptedRestoreHint(change.path)}</p>
          {:else if shown?.error}
            <p class="path-note" data-error>{shown.error}</p>
          {:else if shown}
            {#if shown.text === null}
              <Skeleton lines={3} label="Loading {change.path}" />
            {:else if shown.text === "" && shown.mode === "diff"}
              <p class="path-note">No line changes to show.</p>
            {:else}
              <CheckpointText
                text={shown.text}
                as={shown.mode}
                label={shown.mode === "diff"
                  ? `Changes to ${change.path}`
                  : `${change.path} at this checkpoint`}
              />
            {/if}
          {/if}
        </li>
      {/each}
    </ul>

    <div class="undo">
      <Button icon="restore" loading={undoing} onclick={() => void undo()}
        >Undo these changes</Button
      >
      <p class="undo-hint">
        Puts each file above back as it was before this checkpoint. A file that changed again since
        is left alone.
      </p>
    </div>
    {#if report}
      <Banner tone={report.skipped ? "warn" : "info"} icon={report.skipped ? undefined : "check"}>
        {report.text}
      </Banner>
    {/if}
  {/if}
</div>

<style>
  .changes {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .changes-meta,
  .changes-empty,
  .undo-hint {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .paths {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .path {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-8) 0;
    border-top: 1px solid var(--color-line-soft);
  }

  .path-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-8);
  }

  .path-name {
    flex: 1 1 160px;
    min-width: 0;
    color: var(--color-text);
    font-size: var(--font-size-sm);
    overflow-wrap: anywhere;
  }

  .path-actions {
    display: flex;
    gap: var(--space-4);
    margin-left: auto;
  }

  .path-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);

    &[data-error] {
      color: var(--color-err-text);
    }
  }

  .undo {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-12);
    padding-top: var(--space-4);

    & .undo-hint {
      flex: 1 1 240px;
    }
  }
</style>
