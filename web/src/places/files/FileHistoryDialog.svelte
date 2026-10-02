<script lang="ts">
  import { fetchCheckpointDiff, fetchCheckpointFile, fetchCheckpoints } from "../../lib/api";
  import { triggerLabel } from "../../lib/checkpoints";
  import { configCoordinator } from "../../lib/config-coordinator";
  import { userErrorMessage } from "../../lib/errors";
  import { hub } from "../../lib/hub.svelte";
  import { relativeTime } from "../../lib/time";
  import { toast } from "../../lib/toast.svelte";
  import type { CheckpointSummary } from "../../lib/types";
  import { Banner, Button, Dialog, EmptyState, SegmentedControl, Skeleton } from "../../lib/ui";
  import CheckpointText from "./CheckpointText.svelte";
  import { panelFile } from "./file-buffer.svelte";
  import { fileName, historyLocation, type FileSource } from "./file-source";

  // One file's history: the checkpoints that changed it, newest first, what
  // each changed or the whole file as it was, and restoring a version.

  interface Props {
    source: FileSource;
    path: string;
    onclose: () => void;
    /** A version was restored, so the file is back on disk as it was then. */
    onrestored?: () => void;
  }

  let { source, path, onclose, onrestored }: Props = $props();

  type View = "diff" | "file";
  const VIEWS = [
    { value: "diff", label: "Changes" },
    { value: "file", label: "Whole file" },
  ] as const;

  const name = $derived(fileName(path));
  const location = $derived(historyLocation(source, path));
  const agent = $derived(source.agent === null ? undefined : hub.agent(source.agent));

  let open = $state(true);
  let checkpoints = $state<CheckpointSummary[] | null>(null);
  let loadError = $state("");
  let selectedId = $state<string | null>(null);
  const selected = $derived(checkpoints?.find((c) => c.id === selectedId) ?? null);
  let view = $state<View>("diff");
  /** What the selected version shows: its change or its text, null for no change here, undefined while loading. */
  let shown = $state<string | null | undefined>(undefined);
  let shownError = $state("");
  let restoring = $state(false);

  async function load(): Promise<void> {
    checkpoints = null;
    loadError = "";
    try {
      const page = await fetchCheckpoints(source.agent, { ...location, limit: 100 });
      checkpoints = page.items;
      const newest = page.items[0];
      if (newest !== undefined) void select(newest);
    } catch (err) {
      loadError = userErrorMessage(err, { action: `Couldn't load the history of ${name}.` });
    }
  }

  async function show(checkpoint: CheckpointSummary, as: View): Promise<void> {
    shown = undefined;
    shownError = "";
    const { repo, path: at } = location;
    try {
      const text =
        as === "diff"
          ? await fetchCheckpointDiff(source.agent, checkpoint.id, repo, at)
          : await fetchCheckpointFile(source.agent, checkpoint.id, repo, at);
      if (selectedId === checkpoint.id && view === as) shown = text;
    } catch (err) {
      if (selectedId !== checkpoint.id || view !== as) return;
      const action = as === "diff" ? "Couldn't load what changed." : "Couldn't load this version.";
      shownError = userErrorMessage(err, {
        action,
        notFound: "The file didn't exist at this point.",
      });
    }
  }

  function select(checkpoint: CheckpointSummary): Promise<void> {
    selectedId = checkpoint.id;
    return show(checkpoint, view);
  }

  async function restore(checkpoint: CheckpointSummary): Promise<void> {
    restoring = true;
    try {
      await configCoordinator.restore(source.agent, checkpoint.id, location.repo, location.path);
      toast.success(`Restored ${name} to the version from ${relativeTime(checkpoint.timestamp)}.`);
      onrestored?.();
      if (panelFile.shown?.shows(source, path)) void panelFile.shown.refresh();
      await load();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't restore ${name}.` }));
    } finally {
      restoring = false;
    }
  }

  void load();
</script>

<Dialog
  bind:open
  title="History of {name}"
  description={path.includes("/") ? path : undefined}
  size="lg"
  fullscreenOnPhone
  {onclose}
>
  {#if loadError}
    <Banner tone="error">
      {loadError}
      {#if agent !== undefined && agent.state !== "running"}
        {agent.name} isn't running, and its history may not open until it starts.
      {/if}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load()}>Try again</Button>
      {/snippet}
    </Banner>
  {:else if checkpoints === null}
    <Skeleton lines={4} label="Loading history" />
  {:else if checkpoints.length === 0}
    <EmptyState>No checkpoint has changed this file yet.</EmptyState>
  {:else}
    <ul class="file-history-list" aria-label="Versions">
      {#each checkpoints as checkpoint (checkpoint.id)}
        <li>
          <button
            type="button"
            class="file-history-version"
            aria-current={selectedId === checkpoint.id ? "true" : undefined}
            onclick={() => void select(checkpoint)}
          >
            <span class="file-history-when">{relativeTime(checkpoint.timestamp)}</span>
            <span class="file-history-trigger">{triggerLabel(checkpoint.trigger)}</span>
            <span class="file-history-summary">{checkpoint.summary}</span>
          </button>
        </li>
      {/each}
    </ul>

    {#if selected !== null}
      {@const checkpoint = selected}
      <div class="file-history-detail">
        <div class="file-history-bar">
          <SegmentedControl
            label="Show"
            labelHidden
            options={VIEWS}
            bind:value={view}
            onchange={(as) => void show(checkpoint, as)}
          />
          <Button
            variant="primary"
            size="sm"
            icon="restore"
            loading={restoring}
            onclick={() => void restore(checkpoint)}>Restore this version</Button
          >
        </div>
        {#if shownError}
          <Banner tone="error">{shownError}</Banner>
        {:else if shown === undefined}
          <Skeleton lines={6} />
        {:else if shown === null}
          <EmptyState>{name} didn't change at this checkpoint.</EmptyState>
        {:else}
          <CheckpointText
            text={shown}
            as={view}
            label={view === "diff" ? `What changed in ${name}` : `${name} at this version`}
          />
        {/if}
      </div>
    {/if}
  {/if}
</Dialog>

<style>
  .file-history-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-height: 30vh;
    overflow-y: auto;
    list-style: none;
  }

  .file-history-version {
    display: grid;
    grid-template-columns: 72px 96px minmax(0, 1fr);
    align-items: baseline;
    gap: var(--space-10);
    width: 100%;
    min-height: 32px;
    padding: var(--space-6) var(--space-10);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    text-align: left;
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-4);
    }

    &[aria-current="true"] {
      background: var(--color-vein-tint);

      & .file-history-when,
      & .file-history-trigger {
        color: var(--color-text-2);
      }

      & .file-history-summary {
        color: var(--color-vein-bright);
      }
    }
  }

  .file-history-when,
  .file-history-trigger {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .file-history-summary {
    overflow: hidden;
    color: var(--color-text);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-history-version:hover .file-history-when,
  .file-history-version:hover .file-history-trigger {
    color: var(--color-text-2);
  }

  .file-history-detail {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    margin-top: var(--space-14);
    padding-top: var(--space-14);
    border-top: 1px solid var(--color-line-soft);
  }

  .file-history-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-8);
  }

  @media (max-width: 760px) {
    .file-history-list {
      max-height: none;
    }

    .file-history-version {
      grid-template-columns: auto minmax(0, 1fr);
      min-height: var(--layout-touch-target);

      & .file-history-summary {
        grid-column: 1 / -1;
      }
    }
  }
</style>
