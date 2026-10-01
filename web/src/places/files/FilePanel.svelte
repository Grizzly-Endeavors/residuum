<script lang="ts">
  import { configCoordinator } from "../../lib/config-coordinator";
  import { formatDiagnosticLocation } from "../../lib/diagnostics";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import type { DiagnosticLocation } from "../../lib/types";
  import { Badge, Banner, Button, Dialog, EmptyState, IconButton, Skeleton } from "../../lib/ui";
  import type { WatchHandler, WatchOwner } from "../../lib/watch-registry";
  import { normalizeWatchPrefix } from "../../lib/workspace-watch";
  import { ws } from "../../lib/ws.svelte";
  import PanelHeader from "../../shell/panel/PanelHeader.svelte";
  import { FileBuffer, panelFile } from "./file-buffer.svelte";
  import FileHistoryDialog from "./FileHistoryDialog.svelte";
  import { fileSourceFor, sameSource, type FileSource } from "./file-source";

  // A file in the context panel, from the Files places' trees and from
  // `panel=file:` links: the editor with live validation, Save and Discard,
  // the save conflict, and the file's history. Unsaved edits are asked about
  // before anything closes the panel or shows another file.

  let { source, path }: { source: FileSource; path: string } = $props();

  /** How long after the last keystroke the text is checked: long enough not to fire on every character. */
  const VALIDATE_DEBOUNCE_MS = 500;

  // The panel host keys this by its source, so the buffer is made for one.
  // svelte-ignore state_referenced_locally
  const buffer = new FileBuffer(source);
  let editor = $state<HTMLTextAreaElement>();
  let historyOpen = $state(false);

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
        sameSource(fileSourceFor(target.place), source);
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

  function saveOnShortcut(event: KeyboardEvent): void {
    if ((event.metaKey || event.ctrlKey) && event.key === "s") {
      event.preventDefault();
      void buffer.save();
    }
  }

  /** Put the caret where a diagnostic points, and bring that line into view. */
  function jumpTo(location: DiagnosticLocation): void {
    if (editor === undefined || location.kind === "path") return;
    const lines = buffer.text.split("\n").slice(0, location.line - 1);
    const column = location.kind === "line_column" ? location.column - 1 : 0;
    const offset = lines.reduce((sum, line) => sum + line.length + 1, 0) + column;
    editor.focus();
    editor.setSelectionRange(offset, offset);
    const lineHeight = parseFloat(getComputedStyle(editor).lineHeight);
    if (!Number.isNaN(lineHeight)) editor.scrollTop = Math.max(0, (location.line - 3) * lineHeight);
  }
</script>

<PanelHeader
  icon="file"
  kind="File"
  title={buffer.name}
  code
  meta={inFolder || buffer.dirty ? details : undefined}
  actions={buffer.status === "ready" ? history : undefined}
/>

{#snippet details()}
  {#if inFolder}<code class="file-panel-path">{buffer.path}</code>{/if}
  {#if buffer.dirty}<Badge tone="accent" dot>Unsaved</Badge>{/if}
{/snippet}

{#snippet history()}
  <IconButton
    icon="restore"
    label="History of {buffer.name}"
    onclick={() => (historyOpen = true)}
  />
{/snippet}

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
            >Discard my edits</Button
          >
        {/snippet}
      </Banner>
    {:else if buffer.error}
      <Banner tone="warn" edge ondismiss={() => (buffer.error = "")}>{buffer.error}</Banner>
    {/if}
    <textarea
      class="file-panel-editor"
      aria-label="Contents of {buffer.name}"
      spellcheck="false"
      bind:value={buffer.text}
      bind:this={editor}
      onkeydown={saveOnShortcut}
    ></textarea>
    {#if buffer.diagnostics.length > 0}
      <ul class="file-panel-problems" aria-label="Problems in {buffer.name}">
        {#each buffer.diagnostics as diagnostic, index (index)}
          {@const where = formatDiagnosticLocation(diagnostic.location)}
          <li class="file-panel-problem" data-severity={diagnostic.severity}>
            <Icon name={diagnostic.severity === "error" ? "warning" : "info"} size={14} />
            <span class="file-panel-problem-text">
              {#if diagnostic.location !== undefined && diagnostic.location.kind !== "path"}
                {@const location = diagnostic.location}
                <button type="button" class="file-panel-where" onclick={() => jumpTo(location)}
                  >{where}</button
                >
              {:else if where}
                <span class="file-panel-where">{where}</span>
              {/if}
              {diagnostic.message}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if buffer.dirty}
      <div class="file-panel-save">
        <span class="file-panel-save-note">Unsaved changes</span>
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

  /* The page of the document: the panel's deepest surface, edge to edge under the header. */
  .file-panel-editor {
    flex: 1;
    min-height: 0;
    width: 100%;
    padding: var(--space-14) var(--space-18);
    border: 0;
    background: var(--color-stone-0);
    color: var(--color-text);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-message);
    resize: none;
    tab-size: 2;

    &:focus-visible {
      outline: none;
      box-shadow: inset 2px 0 0 var(--color-vein);
    }
  }

  .file-panel-problems {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: var(--space-4);
    max-height: 30%;
    padding: var(--space-10) var(--space-12);
    overflow-y: auto;
    border-top: 1px solid var(--color-line-soft);
    font-size: var(--font-size-sm);
    list-style: none;
  }

  .file-panel-problem {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
    color: var(--color-text);

    & > :global(svg) {
      flex: none;
      margin-top: var(--space-2);
      color: var(--color-text-2);
    }

    &[data-severity="error"] > :global(svg) {
      color: var(--color-err-text);
    }
  }

  .file-panel-problem-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .file-panel-where {
    margin-right: var(--space-6);
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
  }

  button.file-panel-where {
    color: var(--color-vein-bright);
    text-decoration: underline;
    text-underline-offset: 2px;
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
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  @media (max-width: 760px) {
    .file-panel-editor {
      padding: var(--space-12) var(--space-16);
      font-size: var(--font-size-field-phone);
    }

    .file-panel-save {
      padding-left: var(--space-16);
    }
  }
</style>
