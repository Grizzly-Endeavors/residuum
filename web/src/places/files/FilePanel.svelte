<script lang="ts">
  import { configCoordinator } from "../../lib/config-coordinator";
  import { hub } from "../../lib/hub.svelte";
  import { router } from "../../lib/router.svelte";
  import { Badge, Banner, Button, Dialog, EmptyState, Skeleton } from "../../lib/ui";
  import type { WatchHandler, WatchOwner } from "../../lib/watch-registry";
  import { normalizeWatchPrefix } from "../../lib/workspace-watch";
  import { ws } from "../../lib/ws.svelte";
  import PanelHeader from "../../shell/panel/PanelHeader.svelte";
  import { panelFile, type FileBuffer } from "./file-buffer.svelte";
  import FileHistoryDialog from "./FileHistoryDialog.svelte";
  import FileRendered from "./FileRendered.svelte";
  import { panelFileSource, sameSource } from "./file-source";
  import TextEditor from "./TextEditor.svelte";

  // A file in the context panel, from the Files places' trees and from
  // `panel=file:` links: the editor with live validation, Save and Discard,
  // the save conflict, and the file's history. A Markdown file shows rendered
  // until Edit opens the editor, and the rendered view shows the buffer's text,
  // edits included, with the file's problems in a banner that opens the
  // editor. Unsaved edits are asked about before anything closes the panel or
  // shows another file.

  interface Props {
    /** The open file, which outlives this view while the panel stays open. */
    buffer: FileBuffer;
    /** The path the URL names. */
    path: string;
  }

  let { buffer, path }: Props = $props();

  /** How long after the last keystroke the text is checked: long enough not to fire on every character. */
  const VALIDATE_DEBOUNCE_MS = 500;

  const source = $derived(buffer.source);
  let historyOpen = $state(false);

  const isMarkdown = $derived(/\.md$/i.test(buffer.path));
  const rendered = $derived(isMarkdown && !buffer.editing);

  const inFolder = $derived(buffer.path.includes("/"));

  // Show the URL's file. A rename moved the buffer to its new path first, so the edits stay.
  $effect(() => {
    if (path !== buffer.path) void buffer.open(path);
  });

  $effect(() => {
    panelFile.shown = buffer;
    return () => {
      if (panelFile.shown === buffer) panelFile.shown = null;
    };
  });

  // Leaving with unsaved edits asks first, unless where the user is going shows this same file.
  $effect(() =>
    router.guard.register((target) => {
      if (!buffer.dirty) return null;
      const keeps =
        target?.panel?.kind === "file" &&
        target.panel.path === buffer.path &&
        sameSource(panelFileSource(target.place, target.panel), source);
      return keeps ? null : `Unsaved changes to ${buffer.name}`;
    }),
  );

  // Follow the file on disk: through the agent's socket, or the hub's team
  // watch for the team's folder. After a resync or a reconnect, read it again.
  $effect(() => {
    const prefix = source.scope === "team" ? `team/${buffer.path}` : buffer.path;
    if (buffer.path === "" || normalizeWatchPrefix(prefix) === null) return;
    const refresh = (): void => void buffer.refresh();
    const handler: WatchHandler = { changed: refresh, resync: refresh, reconnected: refresh };
    let owner: WatchOwner | null = null;
    if (source.scope === "team") owner = hub.teamWatches.register(handler);
    else if (source.agent !== null) owner = ws.watches.register(handler, { agent: source.agent });
    owner?.set([prefix]);
    return () => owner?.release();
  });

  // A config file also changes through Settings and the composer's controls.
  $effect(() => {
    const file = buffer.configFile;
    if (file === null) return;
    return configCoordinator.subscribe(file, (change) => {
      if (change.source !== buffer.writer) void buffer.refresh();
    });
  });

  $effect(() => {
    if (buffer.status !== "ready") return;
    void buffer.text;
    const timer = window.setTimeout(() => void buffer.validate(), VALIDATE_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  });

  // Ctrl or Cmd+S saves the open file while it has edits, in either view and
  // from anywhere in the app's window (the header's Edit button included), with
  // the same save as the Save button. Without edits the browser keeps its own.
  function saveOnShortcut(event: KeyboardEvent): void {
    if (!buffer.dirty || !(event.metaKey || event.ctrlKey) || event.key !== "s") return;
    event.preventDefault();
    void buffer.save();
  }

  const problemCount = $derived(buffer.diagnostics.length);
</script>

<PanelHeader
  icon="file"
  kind="File"
  title={buffer.name}
  code
  meta={inFolder ? folder : undefined}
  actions={buffer.status === "ready" ? actions : undefined}
/>

{#snippet folder()}
  <code class="file-panel-path">{buffer.path}</code>
{/snippet}

{#snippet actions()}
  {#if isMarkdown}
    <Button
      class="file-panel-edit"
      variant="quiet"
      size="sm"
      icon="edit"
      pressed={!rendered}
      onclick={() => (buffer.editing = !buffer.editing)}>Edit</Button
    >
  {/if}
  <Button variant="quiet" size="sm" icon="restore" onclick={() => (historyOpen = true)}
    >History</Button
  >
{/snippet}

<svelte:window onkeydown={saveOnShortcut} />

<div class="file-panel">
  {#if buffer.status === "loading"}
    <div class="file-panel-state"><Skeleton lines={8} label="Loading {buffer.name}" /></div>
  {:else if buffer.status === "missing"}
    <div class="file-panel-state">
      <EmptyState variant="block" icon="file" title="This file doesn't exist">
        Nothing is saved at <code>{buffer.path}</code>. It may have been moved or deleted.
      </EmptyState>
    </div>
  {:else if buffer.status === "error"}
    <div class="file-panel-state">
      <Banner tone="error">
        {buffer.error}
        {#snippet actions()}
          <Button size="sm" onclick={() => void buffer.open(buffer.path)}>Try again</Button>
        {/snippet}
      </Banner>
    </div>
  {:else}
    {#if buffer.changedOnDisk !== null}
      <Banner tone="warn" edge>
        {buffer.changedOnDisk === "removed"
          ? `${buffer.name} was deleted while you were editing. Saving writes it again.`
          : `${buffer.name} changed on disk while you were editing. Saving asks which to keep.`}
        {#snippet actions()}
          <Button variant="quiet" size="sm" onclick={() => void buffer.open(buffer.path)}
            >Reload from disk</Button
          >
        {/snippet}
      </Banner>
    {:else if buffer.error}
      <Banner tone="warn" edge ondismiss={() => (buffer.error = "")}>{buffer.error}</Banner>
    {/if}
    {#if rendered}
      {#if problemCount > 0}
        <Banner tone="warn" edge>
          {problemCount === 1
            ? "This file has a problem."
            : `This file has ${problemCount} problems.`}
          {#snippet actions()}
            <Button variant="quiet" size="sm" onclick={() => (buffer.editing = true)}
              >Edit to see them</Button
            >
          {/snippet}
        </Banner>
      {/if}
      <div class="file-panel-rendered">
        {#if buffer.text.trim() === ""}
          <p class="file-panel-empty">Nothing written yet. Edit to add text.</p>
        {:else}
          <FileRendered content={buffer.text} path={buffer.path} {source} mode="markdown" />
        {/if}
      </div>
    {:else}
      <TextEditor
        layout="panel"
        name={buffer.name}
        value={buffer.text}
        problems={buffer.diagnostics}
        oninput={(text) => (buffer.text = text)}
      />
    {/if}
    {#if buffer.dirty}
      <div class="file-panel-save">
        <span class="file-panel-save-note"><Badge tone="accent" dot>Unsaved changes</Badge></span>
        <Button variant="quiet" size="sm" onclick={() => buffer.discard()}>Discard</Button>
        <Button
          variant="primary"
          size="sm"
          loading={buffer.saving}
          onclick={() => void buffer.save()}>Save</Button
        >
      </div>
    {/if}
  {/if}
</div>

<Dialog
  open={buffer.conflict !== null}
  title="{buffer.name} changed on disk"
  description="Something else saved it after you opened it: an agent, another tab, or another program."
  role="alertdialog"
  onclose={() => buffer.answer("cancel")}
>
  Reload it to see what's there now and lose your edits, or overwrite it with your edits.
  {#snippet actions()}
    <Button onclick={() => buffer.answer("use-disk")}>Reload, discard my edits</Button>
    <Button variant="danger" onclick={() => buffer.answer("keep-mine")}
      >Overwrite with my edits</Button
    >
  {/snippet}
</Dialog>

{#if historyOpen}
  <FileHistoryDialog {source} path={buffer.path} onclose={() => (historyOpen = false)} />
{/if}

<style>
  .file-panel {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .file-panel-state {
    padding: var(--space-16) var(--space-18);
  }

  .file-panel-path {
    font-size: var(--font-size-xs);
  }

  .file-panel-rendered {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--space-16) var(--space-18);
  }

  .file-panel-empty {
    margin: 0;
    color: var(--color-text-3);
  }

  .file-panel-save {
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-10) var(--space-12) var(--space-10) var(--space-18);
    border-top: 1px solid var(--color-line-soft);
  }

  .file-panel-save-note {
    flex: 1;
  }

  @media (max-width: 760px) {
    .file-panel-save {
      padding-left: var(--space-16);
    }
  }
</style>
