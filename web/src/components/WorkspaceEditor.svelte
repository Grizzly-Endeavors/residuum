<script lang="ts">
  import type { Diagnostic } from "../lib/types";
  import type { WorkspaceScope } from "../lib/hub-types";
  import {
    fetchWorkspaceFile,
    putWorkspaceFile,
    validateWorkspaceFile,
    workspaceConflictFromApiError,
  } from "../lib/api";
  import { toast } from "../lib/toast.svelte";
  import { userErrorMessage } from "../lib/errors";
  import { formatDiagnosticLocation } from "../lib/diagnostics";
  import Modal from "./Modal.svelte";

  // One workspace file's editor: load, live validation, Save and Discard, and
  // the save-conflict choice. The workspace shows it beside its tree, and the
  // context panel shows it alone. Its owner says which file to show.

  let {
    agent,
    scope = "agent",
    header = true,
    onLoaded,
    onMobileBack,
  }: {
    /** The agent whose file this is. The team scope is no agent's, and takes `null`. */
    agent: string | null;
    scope?: WorkspaceScope;
    /** The bar naming the file. Off where something else already names it. */
    header?: boolean;
    /** A file finished loading. */
    onLoaded?: () => void;
    /** The phone's back button in the bar. */
    onMobileBack?: () => void;
  } = $props();

  /** How long to wait after the last keystroke before validating — long
   * enough to not fire on every character, short enough to feel live. */
  const VALIDATE_DEBOUNCE_MS = 500;

  let selectedFile = $state("");
  let fileContent = $state("");
  let editContent = $state("");
  /** The version the file was last read/saved at, sent back as `If-Match`. */
  let fileVersion = $state<string | null>(null);
  let loading = $state(false);
  let saving = $state(false);
  let error = $state("");
  let diagnostics = $state<Diagnostic[]>([]);
  /** Someone else saved this file first; offer to reload or overwrite. */
  let conflictOpen = $state(false);

  let dirty = $derived(editContent !== fileContent);

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
      void validateWorkspaceFile(agent, path, content, scope).then((result) => {
        diagnostics = result;
      });
    }, VALIDATE_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  /** Whether there are edits that Save hasn't written. */
  export function isDirty(): boolean {
    return dirty;
  }

  /** Show `path`, read from disk, dropping any edits to the file shown. */
  export async function open(path: string): Promise<void> {
    selectedFile = path;
    loading = true;
    error = "";
    diagnostics = [];
    try {
      const file = await fetchWorkspaceFile(agent, path, scope);
      fileContent = file.content;
      editContent = file.content;
      fileVersion = file.version;
      onLoaded?.();
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

  /** Show no file. */
  export function clear(): void {
    selectedFile = "";
    fileContent = "";
    editContent = "";
  }

  /** The file shown was renamed or moved to `path`; the edits stay. */
  export function moved(path: string): void {
    selectedFile = path;
  }

  async function handleSave() {
    if (!selectedFile || !dirty) return;
    saving = true;
    error = "";
    try {
      const response = await putWorkspaceFile(agent, selectedFile, editContent, fileVersion, scope);
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
    await open(selectedFile);
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
      const response = await putWorkspaceFile(agent, selectedFile, editContent, fileVersion, scope);
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

  function fileName(path: string): string {
    const parts = path.split("/");
    return parts[parts.length - 1] || path;
  }
</script>

<div class="workspace-editor">
  {#if selectedFile}
    {#if header}
      <div class="workspace-editor-header">
        <button class="workspace-mobile-back" aria-label="Back to the files" onclick={onMobileBack}
          >&#8592;</button
        >
        <span class="workspace-filename">{fileName(selectedFile)}</span>
      </div>
    {/if}
    {#if loading}
      <div class="workspace-empty">Loading...</div>
    {:else if error}
      <!-- A file that couldn't be opened offers nothing to edit or save. -->
      <div class="workspace-error" role="alert">{error}</div>
    {:else}
      <textarea
        class="workspace-textarea"
        aria-label="Contents of {fileName(selectedFile)}"
        bind:value={editContent}
        spellcheck="false"
      ></textarea>
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
  {:else}
    <div class="workspace-empty">
      <div>No file selected.</div>
    </div>
  {/if}
</div>

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
