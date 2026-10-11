<script lang="ts">
  import { notifyStagedRemoval } from "../../lib/form-undo";
  import { Button, EmptyState, IconButton, TextField } from "../../lib/ui";

  // A list of folders in a config form, with a box to add another. Removing
  // one is a staged change like any other edit: nothing is written until the
  // scope saves, and Discard or the toast's Undo brings it back.

  interface Props {
    paths: string[];
    /** Names the box for a new folder, for assistive technology: "Skill folder to add". */
    addLabel: string;
    placeholder: string;
    /** The line shown while the list is empty. */
    empty: string;
    error?: string;
  }

  let { paths = $bindable(), addLabel, placeholder, empty, error }: Props = $props();

  let draft = $state("");
  let duplicate = $state(false);

  function add(): void {
    const folder = draft.trim();
    if (folder === "") return;
    if (paths.includes(folder)) {
      duplicate = true;
      return;
    }
    paths = [...paths, folder];
    draft = "";
    duplicate = false;
  }

  function remove(index: number): void {
    const removed = paths[index];
    if (removed === undefined) return;
    paths = paths.filter((_, at) => at !== index);
    notifyStagedRemoval(`Removed ${removed}.`, () => {
      if (!paths.includes(removed))
        paths = [...paths.slice(0, index), removed, ...paths.slice(index)];
    });
  }
</script>

{#if paths.length === 0}
  <EmptyState>{empty}</EmptyState>
{:else}
  <ul class="path-list">
    {#each paths as folder, index (index)}
      <li class="path-row">
        <code class="path-name">{folder}</code>
        <IconButton icon="trash" label="Remove {folder}" onclick={() => remove(index)} />
      </li>
    {/each}
  </ul>
{/if}
<form
  class="path-add"
  onsubmit={(event) => {
    event.preventDefault();
    add();
  }}
>
  <div class="path-add-box">
    <TextField
      label={addLabel}
      labelHidden
      code
      bind:value={draft}
      {placeholder}
      autocomplete="off"
      spellcheck={false}
      error={duplicate ? "That folder is already in the list." : error}
      oninput={() => {
        duplicate = false;
      }}
    />
  </div>
  <Button type="submit" icon="plus" disabled={draft.trim() === ""}>Add</Button>
</form>

<style>
  .path-list {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .path-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
    padding: var(--space-2) 0;
    border-top: 1px solid var(--color-line-soft);
  }

  .path-row:first-child {
    border-top: 0;
  }

  .path-name {
    min-width: 0;
    font-size: var(--font-size-sm);
    color: var(--color-text);
    overflow-wrap: anywhere;
  }

  .path-add {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
  }

  .path-add-box {
    flex: 1;
    min-width: 0;
  }
</style>
