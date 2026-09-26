<script lang="ts">
  import type { WorkspaceEntry } from "../lib/types";
  import { focusOnMount } from "../lib/actions/focusOnMount";

  interface TreeItem {
    entry: WorkspaceEntry;
    path: string;
    depth: number;
  }

  let {
    items,
    selectedFile,
    expandedDirs,
    onSelectFile,
    onToggleDir,
    onDeleteFile,
    onRenameFile,
    onShowHistory,
  }: {
    items: TreeItem[];
    selectedFile: string;
    expandedDirs: Set<string>;
    onSelectFile: (path: string) => void;
    onToggleDir: (path: string) => void;
    onDeleteFile: (path: string) => void;
    onRenameFile: (path: string, newName: string) => void;
    onShowHistory: (path: string) => void;
  } = $props();

  const IDENTITY_FILES = new Set([
    "SOUL.md",
    "AGENTS.md",
    "USER.md",
    "HEARTBEAT.yml",
    "CHANNELS.yml",
  ]);

  function isIdentity(name: string): boolean {
    return IDENTITY_FILES.has(name);
  }

  // Renaming one file at a time, by path.
  let renamingPath = $state<string | null>(null);
  let renameValue = $state("");

  function startRename(item: TreeItem): void {
    renamingPath = item.path;
    renameValue = item.entry.name;
  }

  function cancelRename(): void {
    renamingPath = null;
    renameValue = "";
  }

  function commitRename(item: TreeItem): void {
    const name = renameValue.trim();
    renamingPath = null;
    if (!name || name === item.entry.name) return;
    onRenameFile(item.path, name);
  }
</script>

<div class="workspace-tree">
  {#each items as item (item.path)}
    {#if item.entry.entry_type === "directory"}
      <button
        class="tree-entry tree-dir"
        style="padding-left: {12 + item.depth * 16}px"
        onclick={() => onToggleDir(item.path)}
      >
        <span class="tree-dir-chevron" class:open={expandedDirs.has(item.path)}>&#9656;</span>
        <span class="tree-entry-name">{item.entry.name}</span>
      </button>
    {:else if renamingPath === item.path}
      <form
        class="tree-entry tree-entry-rename"
        style="padding-left: {12 + item.depth * 16}px"
        onsubmit={(e) => {
          e.preventDefault();
          commitRename(item);
        }}
      >
        <input
          class="tree-rename-input"
          type="text"
          bind:value={renameValue}
          use:focusOnMount
          onblur={cancelRename}
          onkeydown={(e) => {
            if (e.key === "Escape") cancelRename();
          }}
        />
      </form>
    {:else}
      <div class="tree-entry-row">
        <button
          class="tree-entry"
          class:active={selectedFile === item.path}
          class:identity={isIdentity(item.entry.name)}
          style="padding-left: {12 + item.depth * 16}px"
          onclick={() => onSelectFile(item.path)}
        >
          <span class="tree-entry-name">{item.entry.name}</span>
        </button>
        <div class="tree-entry-actions">
          <button
            type="button"
            class="tree-entry-action"
            title="View this file's history"
            aria-label="View {item.entry.name}'s history"
            onclick={() => onShowHistory(item.path)}
          >
            History
          </button>
          <button
            type="button"
            class="tree-entry-action"
            title="Rename or move {item.entry.name}"
            aria-label="Rename {item.entry.name}"
            onclick={() => startRename(item)}
          >
            Rename
          </button>
          <button
            type="button"
            class="tree-entry-action tree-entry-action-danger"
            title="Delete {item.entry.name}"
            aria-label="Delete {item.entry.name}"
            onclick={() => onDeleteFile(item.path)}
          >
            Delete
          </button>
        </div>
      </div>
    {/if}
  {/each}
  {#if items.length === 0}
    <div class="tree-empty">Empty directory</div>
  {/if}
</div>
