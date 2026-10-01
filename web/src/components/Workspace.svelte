<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import type { WorkspaceChange, WorkspaceEntry } from "../lib/types";
  import type { WorkspaceScope } from "../lib/hub-types";
  import { fetchWorkspaceFiles, deleteWorkspaceFile, moveWorkspaceFile } from "../lib/api";
  import { toast } from "../lib/toast.svelte";
  import { userErrorMessage } from "../lib/errors";
  import { notifyWithWorkspaceUndo } from "../lib/undo";
  import { ws } from "../lib/ws.svelte";
  import { hub } from "../lib/hub.svelte";
  import { treeUpdateFor, treeWatchPrefix } from "../lib/tree-changes";
  import type { WatchOwner } from "../lib/watch-registry";
  import FileTree from "./FileTree.svelte";
  import FileHistoryModal from "./FileHistoryModal.svelte";
  import Modal from "./Modal.svelte";
  import WorkspaceEditor from "./WorkspaceEditor.svelte";

  let {
    agent,
    scope = "agent",
  }: {
    /** The agent whose tree this shows. The team scope is no agent's, and takes `null`. */
    agent: string | null;
    scope?: WorkspaceScope;
  } = $props();

  let editor = $state<WorkspaceEditor>();
  let selectedFile = $state("");
  let error = $state("");
  let expandedDirs = new SvelteSet<string>();
  let treeCache = $state<Record<string, WorkspaceEntry[]>>({});
  let mobileEditorOpen = $state(false);
  let switchConfirmOpen = $state(false);
  let pendingFilePath = $state("");
  let historyPath = $state<string | null>(null);

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
    await refreshDir(path);
  }

  function parentDir(path: string): string {
    const slash = path.lastIndexOf("/");
    return slash < 0 ? "" : path.slice(0, slash);
  }

  /** List a directory again and show that in place of its cached listing, so the tree reflects
   * a delete, move, restore or change made outside the normal `loadFile`/`handleSave` flow. */
  async function refreshDir(path: string): Promise<void> {
    try {
      const entries = await fetchWorkspaceFiles(agent, path || undefined, scope);
      treeCache = { ...treeCache, [path]: entries };
      error = "";
    } catch (e) {
      error = userErrorMessage(e, {
        action: "Couldn't list this folder.",
        notFound: "It may have been moved or deleted.",
      });
    }
  }

  /** Forget the listings of folders that no longer exist. */
  function forgetDirs(dirs: readonly string[]): void {
    if (dirs.length === 0) return;
    treeCache = Object.fromEntries(
      Object.entries(treeCache).filter(([dir]) => !dirs.includes(dir)),
    );
    for (const dir of dirs) expandedDirs.delete(dir);
  }

  // The tree follows the disk: it owns a watch on its whole tree, through the
  // agent's socket or, for the team tree, the hub's team watch, and lists again
  // the folders that gain or lose something. The registry keeps it from
  // touching anyone else's watch.
  $effect(() => {
    const handler = {
      changed: (changes: WorkspaceChange[]) => {
        const update = treeUpdateFor(changes, scope, Object.keys(treeCache));
        forgetDirs(update.forget);
        for (const dir of update.reload) void refreshDir(dir);
      },
      // The feed lost track of changes: nothing listed can be trusted.
      resync: () => {
        for (const dir of Object.keys(treeCache)) void refreshDir(dir);
      },
    };
    let owner: WatchOwner | null = null;
    if (scope === "team") owner = hub.teamWatches.register(handler);
    else if (agent !== null) owner = ws.watches.register(handler, { agent });
    owner?.set([treeWatchPrefix(scope)]);
    return () => owner?.release();
  });

  function clearEditorIfOpen(path: string): void {
    if (selectedFile !== path) return;
    selectedFile = "";
    editor?.clear();
  }

  async function handleDeleteFile(path: string): Promise<void> {
    try {
      const checkpoints = await deleteWorkspaceFile(agent, path, scope);
      clearEditorIfOpen(path);
      await refreshDir(parentDir(path));
      notifyWithWorkspaceUndo(
        agent,
        `Deleted ${fileName(path)}.`,
        scope === "team" ? `team/${path}` : path,
        checkpoints,
        () => refreshDir(parentDir(path)),
      );
    } catch (e) {
      toast.error(userErrorMessage(e, { action: `Couldn't delete ${fileName(path)}.` }));
    }
  }

  async function handleRenameFile(path: string, newName: string): Promise<void> {
    const dir = parentDir(path);
    const to = dir ? `${dir}/${newName}` : newName;
    try {
      await moveWorkspaceFile(agent, path, to, false, scope);
      if (selectedFile === path) {
        selectedFile = to;
        editor?.moved(to);
      }
      await refreshDir(dir);
      toast.success(`Renamed to ${newName}.`);
    } catch (e) {
      toast.error(userErrorMessage(e, { action: `Couldn't rename ${fileName(path)}.` }));
    }
  }

  function handleShowHistory(path: string): void {
    historyPath = path;
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
    if (editor?.isDirty()) {
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
    await editor?.open(path);
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
      onDeleteFile={(path) => void handleDeleteFile(path)}
      onRenameFile={(path, name) => void handleRenameFile(path, name)}
      onShowHistory={handleShowHistory}
    />
    {#if error}
      <div class="workspace-error">{error}</div>
    {/if}
  </div>

  <WorkspaceEditor
    bind:this={editor}
    {agent}
    {scope}
    onLoaded={() => (mobileEditorOpen = true)}
    onMobileBack={handleMobileBack}
  />
</div>

<Modal open={switchConfirmOpen} title="Discard unsaved changes?" onClose={cancelSwitchFile}>
  Switching files will discard your unsaved edits to {fileName(selectedFile)}.

  {#snippet actions()}
    <button class="btn btn-secondary" onclick={cancelSwitchFile}>Cancel</button>
    <button class="btn btn-danger" onclick={confirmSwitchFile}>Discard and switch</button>
  {/snippet}
</Modal>

{#if historyPath}
  <FileHistoryModal
    path={historyPath}
    {agent}
    {scope}
    onClose={() => {
      historyPath = null;
    }}
    onRestored={() => {
      const path = historyPath;
      if (!path) return;
      void refreshDir(parentDir(path));
      if (selectedFile === path) void loadFile(path);
    }}
  />
{/if}
