<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import type { WorkspaceEntry } from "../lib/types";
  import { fetchWorkspaceFiles, fetchWorkspaceFile, putWorkspaceFile } from "../lib/api";
  import { toast } from "../lib/toast.svelte";
  import { Icon } from "../lib/icons";
  import FileTree from "./FileTree.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // State
  let selectedFile = $state("");
  let fileContent = $state("");
  let editContent = $state("");
  let loading = $state(false);
  let saving = $state(false);
  let error = $state("");
  let expandedDirs = new SvelteSet<string>();
  let treeCache = $state<Record<string, WorkspaceEntry[]>>({});
  let mobileEditorOpen = $state(false);

  // Derived
  let dirty = $derived(editContent !== fileContent);

  // Flatten tree into items with depth for FileTree
  interface TreeItem {
    entry: WorkspaceEntry;
    path: string;
    depth: number;
  }

  let treeItems = $derived.by(() => {
    const result: TreeItem[] = [];
    function addEntries(dirPath: string, depth: number) {
      const entries = treeCache[dirPath];
      if (!entries) return;
      for (const entry of entries) {
        const path = dirPath ? `${dirPath}/${entry.name}` : entry.name;
        result.push({ entry, path, depth });
        if (entry.entry_type === "directory" && expandedDirs.has(path)) {
          addEntries(path, depth + 1);
        }
      }
    }
    addEntries("", 0);
    return result;
  });

  onMount(() => {
    void loadDir("");
  });

  async function loadDir(path: string) {
    if (treeCache[path]) return;
    try {
      const entries = await fetchWorkspaceFiles(path || undefined);
      treeCache = { ...treeCache, [path]: entries };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function handleToggleDir(path: string) {
    if (expandedDirs.has(path)) {
      expandedDirs.delete(path);
    } else {
      expandedDirs.add(path);
      await loadDir(path);
    }
  }

  async function handleSelectFile(path: string) {
    selectedFile = path;
    loading = true;
    error = "";
    try {
      const content = await fetchWorkspaceFile(path);
      fileContent = content;
      editContent = content;
      mobileEditorOpen = true;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      fileContent = "";
      editContent = "";
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!selectedFile || !dirty) return;
    saving = true;
    error = "";
    try {
      await putWorkspaceFile(selectedFile, editContent);
      fileContent = editContent;
      toast.success("Saved.");
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      toast.error(`Save failed. ${msg}`);
    } finally {
      saving = false;
    }
  }

  function handleDiscard() {
    editContent = fileContent;
  }

  function handleMobileBack() {
    mobileEditorOpen = false;
  }

  function fileName(path: string): string {
    const parts = path.split("/");
    return parts[parts.length - 1] || path;
  }
</script>

<div class="workspace-view" class:mobile-editor-open={mobileEditorOpen}>
  <div class="workspace-tree-pane">
    <FileTree
      items={treeItems}
      {selectedFile}
      {expandedDirs}
      onSelectFile={handleSelectFile}
      onToggleDir={handleToggleDir}
    />
  </div>

  <div class="workspace-editor">
    {#if selectedFile}
      <div class="workspace-editor-header">
        <button class="workspace-mobile-back" onclick={handleMobileBack}>&#8592;</button>
        <span class="workspace-filename">{fileName(selectedFile)}</span>
        <button class="workspace-close" onclick={onClose} title="Close workspace">&#10005;</button>
      </div>
      {#if loading}
        <div class="workspace-empty">Loading...</div>
      {:else}
        <textarea class="workspace-textarea" bind:value={editContent} spellcheck="false"></textarea>
        <div class="workspace-footer">
          <span class="workspace-file-info">
            {selectedFile}
            {#if dirty}
              <span class="workspace-dirty-badge">modified</span>
            {/if}
          </span>
          {#if dirty}
            <div class="workspace-footer-actions">
              <button class="btn btn-secondary btn-sm" onclick={handleDiscard}>Discard</button>
              <button class="btn btn-primary btn-sm" onclick={handleSave} disabled={saving}>
                {saving ? "Saving..." : "Save"}
              </button>
            </div>
          {/if}
        </div>
      {/if}
      {#if error}
        <div class="workspace-error">{error}</div>
      {/if}
    {:else}
      <div class="workspace-empty">
        <div>No file selected.</div>
        <button
          class="workspace-close workspace-close-empty"
          onclick={onClose}
          title="Close workspace"
          aria-label="Close workspace"
        >
          <Icon name="close" size={14} />
        </button>
      </div>
    {/if}
  </div>
</div>
