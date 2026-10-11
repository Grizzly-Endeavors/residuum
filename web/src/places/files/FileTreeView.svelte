<script lang="ts">
  import { tick } from "svelte";
  import { focusOnMount } from "../../lib/actions/focusOnMount";
  import { Icon } from "../../lib/icons";
  import { relativeTime } from "../../lib/time";
  import { Button, IconButton, Menu, MenuItem } from "../../lib/ui";
  import { configFileAt, fileName, isIdentityFile } from "./file-source";
  import type { FileTree } from "./file-tree.svelte";

  // The rows of a file tree: folders that open and close, files that open in
  // the context panel, and each file's History, Rename and Delete.

  interface Props {
    tree: FileTree;
    /** The path the context panel shows, which the tree marks. */
    shown: string | null;
    onopen: (path: string) => void;
    onhistory: (path: string) => void;
    /** A file was renamed or moved from `from` to `to`. */
    onmoved: (from: string, to: string) => void;
  }

  let { tree, shown, onopen, onhistory, onmoved }: Props = $props();

  let list = $state<HTMLElement>();
  let renaming = $state<string | null>(null);
  let newName = $state("");

  function startRename(path: string): void {
    renaming = path;
    newName = fileName(path);
  }

  async function commitRename(path: string): Promise<void> {
    renaming = null;
    const to = await tree.rename(path, newName);
    if (to === null) return;
    onmoved(path, to);
    await tick();
    list?.querySelector<HTMLElement>(`[data-path="${CSS.escape(to)}"]`)?.focus();
  }
</script>

<ul class="file-tree" aria-label="Files" bind:this={list}>
  {#each tree.rows as row (`${row.kind}:${row.path}`)}
    <li class="file-tree-item" style:--file-tree-depth={row.depth}>
      {#if row.kind === "folder"}
        {@const open = tree.expanded.has(row.path)}
        <button
          type="button"
          class="file-tree-row"
          aria-expanded={open}
          data-path={row.path}
          onclick={() => void tree.toggle(row.path)}
        >
          <span class="file-tree-chevron" class:is-open={open}
            ><Icon name="chevron-right" size={13} /></span
          >
          <Icon name="folder" size={15} />
          <span class="file-tree-name">{row.entry.name}</span>
        </button>
      {:else if row.kind === "file" && renaming === row.path}
        <form
          class="file-tree-rename"
          onsubmit={(event) => {
            event.preventDefault();
            void commitRename(row.path);
          }}
        >
          <input
            class="file-tree-rename-input"
            aria-label="New name for {row.entry.name}"
            spellcheck="false"
            bind:value={newName}
            use:focusOnMount
            onblur={() => (renaming = null)}
            onkeydown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                renaming = null;
              }
            }}
          />
        </form>
      {:else if row.kind === "file"}
        {@const name = row.entry.name}
        <div
          class="file-tree-file"
          class:is-shown={shown === row.path}
          data-identity={isIdentityFile(tree.source, row.path) || undefined}
        >
          <button
            type="button"
            class="file-tree-row"
            aria-current={shown === row.path ? "true" : undefined}
            data-path={row.path}
            onclick={() => onopen(row.path)}
          >
            <Icon name="file" size={14} />
            <span class="file-tree-name">{name}</span>
            <span class="file-tree-meta">{relativeTime(new Date(row.entry.modified))}</span>
          </button>
          <Menu label="Manage {name}" align="end">
            {#snippet trigger(props)}
              <IconButton icon="more" label="More for {name}" size="sm" {...props} />
            {/snippet}
            <MenuItem label="History" icon="restore" onselect={() => onhistory(row.path)} />
            <!-- An agent's config files are written only through the config write
              coordinator, which neither moves nor deletes. A delete also couldn't be
              undone: its checkpoint is the workspace's, which leaves config.toml and
              providers.toml out. -->
            {#if configFileAt(tree.source, row.path) === null}
              <MenuItem label="Rename" icon="edit" onselect={() => startRename(row.path)} />
              <MenuItem
                label="Delete"
                icon="close"
                tone="danger"
                onselect={() => void tree.remove(row.path)}
              />
            {/if}
          </Menu>
        </div>
      {:else if row.kind === "empty"}
        <p class="file-tree-note">Empty</p>
      {:else if row.kind === "error"}
        <div class="file-tree-note is-error" role="alert">
          <span>{row.message}</span>
          <Button variant="quiet" size="sm" onclick={() => void tree.list(row.path)}
            >Try again</Button
          >
        </div>
      {/if}
    </li>
  {/each}
</ul>

<style>
  .file-tree {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    font-size: var(--font-size-sm);
    list-style: none;
  }

  .file-tree-item {
    padding-left: calc(var(--space-18) * var(--file-tree-depth));
  }

  .file-tree-row {
    display: flex;
    flex: 1;
    align-items: center;
    gap: var(--space-8);
    width: 100%;
    min-width: 0;
    min-height: 32px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    text-align: left;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-2);
      color: var(--color-text);
    }

    &[aria-expanded] {
      color: var(--color-text);
    }

    & > :global(svg) {
      flex: none;
    }
  }

  .file-tree-chevron {
    display: grid;
    flex: none;
    place-items: center;
    color: var(--color-text-3);
    transition: transform var(--duration-base) var(--ease-out);

    &.is-open {
      transform: rotate(90deg);
    }
  }

  .file-tree-name {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .file-tree-file {
    display: flex;
    align-items: center;
    border-radius: var(--corner-sm);

    & .file-tree-name {
      font-family: var(--font-code);
      font-size: var(--font-size-xs);
    }

    /* Identity files: the agent's soul and heartbeat, the team's rules and user facts. */
    &[data-identity] .file-tree-row {
      color: var(--color-vein-bright);
    }

    &.is-shown {
      background: var(--color-vein-tint);

      & .file-tree-row {
        color: var(--color-vein-bright);
      }

      & .file-tree-meta {
        color: var(--color-text-2);
      }

      & .file-tree-row:hover {
        background: transparent;
      }
    }

    /* The row's menu stays out of the way until the row is in play. */
    & > :global(.ui-icon-button) {
      flex: none;
      margin-right: var(--space-4);
      opacity: 0;
    }

    &:hover > :global(.ui-icon-button),
    &:focus-within > :global(.ui-icon-button),
    & > :global(.ui-icon-button[aria-expanded="true"]) {
      opacity: 1;
    }
  }

  .file-tree-meta {
    flex: none;
    margin-left: auto;
    padding-left: var(--space-8);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    white-space: nowrap;
  }

  .file-tree-rename {
    display: flex;
    padding: var(--space-2) 0;
  }

  .file-tree-rename-input {
    flex: 1;
    min-width: 0;
    min-height: 30px;
    padding: 0 var(--space-10);
    border: 1px solid var(--color-vein);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    color: var(--color-text);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);

    &:focus-visible {
      outline: none;
      box-shadow: 0 0 0 3px var(--color-vein-faint);
    }
  }

  .file-tree-note {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-10);
    padding: var(--space-6) var(--space-10) var(--space-6) var(--space-32);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);

    &.is-error {
      color: var(--color-err-text);
    }
  }

  /* Touch has no hover: the menu shows on every row, and rows reach the touch target. */
  @media (hover: none) {
    .file-tree-file > :global(.ui-icon-button) {
      opacity: 1;
    }
  }

  @media (max-width: 760px) {
    .file-tree-row {
      min-height: var(--layout-touch-target);
    }

    .file-tree-rename-input {
      min-height: var(--layout-touch-target);
      font-size: var(--font-size-field-phone);
    }
  }
</style>
