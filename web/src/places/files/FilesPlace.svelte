<script lang="ts">
  import { onMount } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import { router } from "../../lib/router.svelte";
  import { EmptyState, Skeleton } from "../../lib/ui";
  import FileHistoryDialog from "./FileHistoryDialog.svelte";
  import { parentDir, type FileSource } from "./file-source";
  import { FileTree } from "./file-tree.svelte";
  import FileTreeView from "./FileTreeView.svelte";

  // Files (an agent's folder) and Shared files (the team's): the tree, kept
  // current from the disk. A file opens in the context panel's editor.

  let { source }: { source: FileSource } = $props();

  // The shell keys this place by its source, so the tree is made for one.
  // svelte-ignore state_referenced_locally
  const tree = new FileTree(source);
  let historyPath = $state<string | null>(null);

  const shown = $derived(router.panel?.kind === "file" ? router.panel.path : null);
  const agent = $derived(source.agent === null ? undefined : hub.agent(source.agent));
  const root = $derived(tree.listings[""]);

  onMount(() => {
    const linked = shown;
    void tree.list("").then(() => (linked === null ? undefined : tree.reveal(linked)));
  });
  $effect(() => tree.watch());

  function open(path: string): void {
    void router.openPanel({ kind: "file", path });
  }

  /** The panel follows a file it shows to where it was renamed or moved. */
  function moved(from: string, to: string): void {
    if (shown === from) void router.replacePanel({ kind: "file", path: to });
  }
</script>

<div class="files-scroll">
  <div class="files">
    <p class="files-intro">
      {#if source.agent === null}
        Files every agent can read and write.
      {:else}
        {source.agent}'s memory, skills and notes. Its <code>team</code> folder holds the
        <button
          type="button"
          class="files-link"
          onclick={() => void router.openPlace({ kind: "shared-files" })}>shared files</button
        >, the same for every agent.
      {/if}
    </p>
    {#if agent !== undefined && agent.state !== "running"}
      <p class="files-intro">
        {agent.name} isn't running. Changes saved here are read when it next starts.
      </p>
    {/if}

    {#if root === undefined && !("" in tree.errors)}
      <Skeleton lines={6} width="60%" label="Loading files" />
    {:else if root?.length === 0}
      <EmptyState variant="block" icon="folder" title="Nothing here yet">
        {source.agent === null
          ? "Files the agents share show up here as they write them."
          : `Files ${source.agent} writes show up here.`}
      </EmptyState>
    {:else}
      <FileTreeView
        {tree}
        {shown}
        onopen={open}
        onhistory={(path) => (historyPath = path)}
        onmoved={moved}
      />
    {/if}
  </div>
</div>

{#if historyPath !== null}
  {@const path = historyPath}
  <FileHistoryDialog
    {source}
    {path}
    onclose={() => (historyPath = null)}
    onrestored={() => void tree.relist(parentDir(path))}
  />
{/if}

<style>
  .files-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .files {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    max-width: 880px;
    padding: var(--space-20) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
  }

  .files-intro {
    max-width: var(--layout-reading-width);
    color: var(--color-text-3);
    font-size: var(--font-size-sm);

    & + .files-intro {
      margin-top: calc(var(--space-8) * -1);
    }

    & code {
      color: var(--color-text-2);
    }
  }

  .files-link {
    color: var(--color-vein-bright);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  @media (max-width: 760px) {
    .files {
      padding: var(--space-16) var(--space-16) var(--space-40);
    }
  }
</style>
