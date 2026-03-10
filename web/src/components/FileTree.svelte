<script lang="ts">
  import type { WorkspaceEntry } from "../lib/types";

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
  }: {
    items: TreeItem[];
    selectedFile: string;
    expandedDirs: Set<string>;
    onSelectFile: (path: string) => void;
    onToggleDir: (path: string) => void;
  } = $props();

  const IDENTITY_FILES = new Set([
    "SOUL.md",
    "AGENTS.md",
    "USER.md",
    "MEMORY.md",
    "ENVIRONMENT.md",
    "PRESENCE.toml",
    "HEARTBEAT.yml",
    "CHANNELS.yml",
  ]);

  function isIdentity(name: string): boolean {
    return IDENTITY_FILES.has(name);
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
    {:else}
      <button
        class="tree-entry"
        class:active={selectedFile === item.path}
        class:identity={isIdentity(item.entry.name)}
        style="padding-left: {12 + item.depth * 16}px"
        onclick={() => onSelectFile(item.path)}
      >
        <span class="tree-entry-name">{item.entry.name}</span>
      </button>
    {/if}
  {/each}
  {#if items.length === 0}
    <div class="tree-empty">Empty directory</div>
  {/if}
</div>
