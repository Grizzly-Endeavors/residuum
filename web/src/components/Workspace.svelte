<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import type { WorkspaceEntry, Diagnostic } from "../lib/types";
  import {
    fetchWorkspaceFiles,
    fetchWorkspaceFile,
    putWorkspaceFile,
    validateWorkspaceFile,
    workspaceConflictFromApiError,
  } from "../lib/api";
  import { toast } from "../lib/toast.svelte";
  import { Icon } from "../lib/icons";
  import { userErrorMessage } from "../lib/errors";
  import { formatDiagnosticLocation } from "../lib/diagnostics";
  import FileTree from "./FileTree.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  /** How long to wait after the last keystroke before validating — long
   * enough to not fire on every character, short enough to feel live. */
  const VALIDATE_DEBOUNCE_MS = 500;

  // State
  let selectedFile = $state("");
  let fileContent = $state("");
  let editContent = $state("");
  /** The version the file was last read/saved at, sent back as `If-Match`. */
  let fileVersion = $state<string | null>(null);
  let loading = $state(false);
  let saving = $state(false);
  let error = $state("");
  let diagnostics = $state<Diagnostic[]>([]);
  let expandedDirs = new SvelteSet<string>();
  let treeCache = $state<Record<string, WorkspaceEntry[]>>({});
  let mobileEditorOpen = $state(false);
  let switchConfirmOpen = $state(false);
  let pendingFilePath = $state("");
  /** Someone else saved this file first; offer to reload or overwrite. */
  let conflictOpen = $state(false);

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

  // Debounced live validation: re-checks `editContent` shortly after each
  // change. Diagnostics for an unrecognized path just come back empty, so
  // this runs unconditionally rather than special-casing which files matter.
  $effect(() => {
    const path = selectedFile;
    const content = editContent;
    if (!path) {
      diagnostics = [];
      return;
    }
    const timer = setTimeout(() => {
      void validateWorkspaceFile(path, content).then((result) => {
        diagnostics = result;
      });
    }, VALIDATE_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  async function loadDir(path: string) {
    if (treeCache[path]) return;
    try {
      const entries = await fetchWorkspaceFiles(path || undefined);
      treeCache = { ...treeCache, [path]: entries };
    } catch (e) {
      error = userErrorMessage(e, {
        action: "Couldn't list this folder.",
        notFound: "It may have been moved or deleted.",
      });
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
    if (dirty) {
      pendingFilePath = path;
      switchConfirmOpen = true;
      return;
    }
    await loadFile(path);
  }

  function cancelSwitchFile() {
    switchConfirmOpen = false;
    pendingFilePath = "";
  }

  async function confirmSwitchFile() {
    switchConfirmOpen = false;
    const path = pendingFilePath;
    pendingFilePath = "";
    await loadFile(path);
  }

  async function loadFile(path: string) {
    selectedFile = path;
    loading = true;
    error = "";
    diagnostics = [];
    try {
      const file = await fetchWorkspaceFile(path);
      fileContent = file.content;
      editContent = file.content;
      fileVersion = file.version;
      mobileEditorOpen = true;
    } catch (e) {
      error = userErrorMessage(e, {
        action: "Couldn't open this file.",
        notFound: "It may have been moved or deleted.",
      });
      fileContent = "";
      editContent = "";
      fileVersion = null;
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!selectedFile || !dirty) return;
    saving = true;
    error = "";
    try {
      const response = await putWorkspaceFile(selectedFile, editContent, fileVersion);
      fileContent = editContent;
      fileVersion = response.version;
      diagnostics = response.diagnostics ?? [];
      toast.success(diagnostics.length > 0 ? "Saved, with problems noted below." : "Saved.");
    } catch (e) {
      const conflict = workspaceConflictFromApiError(e);
      if (conflict) {
        fileVersion = conflict.currentVersion;
        conflictOpen = true;
      } else {
        toast.error(userErrorMessage(e, { action: "Couldn't save this file." }));
      }
    } finally {
      saving = false;
    }
  }

  /** Reload the file's current content from disk, discarding local edits. */
  async function resolveConflictByReloading() {
    conflictOpen = false;
    await loadFile(selectedFile);
  }

  /**
   * Overwrite the other writer's change with this one. `fileVersion` was
   * already updated to the conflict's `current_version` in `handleSave`,
   * so this still goes through `If-Match` against exactly what's on disk
   * now — a further concurrent change in the meantime still 412s rather
   * than being silently clobbered too.
   */
  async function resolveConflictByOverwriting() {
    conflictOpen = false;
    saving = true;
    try {
      const response = await putWorkspaceFile(selectedFile, editContent, fileVersion);
      fileContent = editContent;
      fileVersion = response.version;
      diagnostics = response.diagnostics ?? [];
      toast.success(
        diagnostics.length > 0
          ? "Saved (overwrote the other change), with problems noted below."
          : "Saved (overwrote the other change).",
      );
    } catch (e) {
      toast.error(userErrorMessage(e, { action: "Couldn't save this file." }));
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
        {#if diagnostics.length > 0}
          <ul class="workspace-diagnostics">
            {#each diagnostics as diagnostic, i (i)}
              <li class="workspace-diagnostic workspace-diagnostic-{diagnostic.severity}">
                <span class="workspace-diagnostic-severity">{diagnostic.severity}</span>
                {#if diagnostic.location}
                  <span class="workspace-diagnostic-location"
                    >{formatDiagnosticLocation(diagnostic.location)}</span
                  >
                {/if}
                <span class="workspace-diagnostic-message">{diagnostic.message}</span>
              </li>
            {/each}
          </ul>
        {/if}
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

<Modal open={switchConfirmOpen} title="Discard unsaved changes?" onClose={cancelSwitchFile}>
  Switching files will discard your unsaved edits to {fileName(selectedFile)}.

  {#snippet actions()}
    <button class="btn btn-secondary" onclick={cancelSwitchFile}>Cancel</button>
    <button class="btn btn-danger" onclick={confirmSwitchFile}>Discard and switch</button>
  {/snippet}
</Modal>

<Modal open={conflictOpen} title="This file changed" onClose={() => (conflictOpen = false)}>
  {fileName(selectedFile)} was saved by someone else (or something else) since you opened it. Reload to
  see the current version and lose your edits, or overwrite it with your edits.

  {#snippet actions()}
    <button class="btn btn-secondary" onclick={() => void resolveConflictByReloading()}
      >Reload, discard my edits</button
    >
    <button class="btn btn-danger" onclick={() => void resolveConflictByOverwriting()}
      >Overwrite with my edits</button
    >
  {/snippet}
</Modal>
