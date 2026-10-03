<script lang="ts">
  import { notifyStagedRemoval } from "../../lib/form-undo";
  import { Button, EmptyState, IconButton, TextField } from "../../lib/ui";

  // A list of plain-language rules in a config form, with a box to add
  // another. Removing one is a staged change like any other edit: nothing is
  // written until the scope saves, and Discard or the toast's Undo brings it
  // back.

  interface Props {
    rules: string[];
    /** Names the box for a new rule, for assistive technology: "Deny rule to add". */
    addLabel: string;
    placeholder: string;
    /** The line shown while the list is empty. */
    empty: string;
    error?: string;
  }

  let { rules = $bindable(), addLabel, placeholder, empty, error }: Props = $props();

  let draft = $state("");
  let duplicate = $state(false);

  function add(): void {
    const rule = draft.trim();
    if (rule === "") return;
    if (rules.includes(rule)) {
      duplicate = true;
      return;
    }
    rules = [...rules, rule];
    draft = "";
    duplicate = false;
  }

  function remove(index: number): void {
    const removed = rules[index];
    if (removed === undefined) return;
    rules = rules.filter((_, at) => at !== index);
    notifyStagedRemoval(`Removed “${removed}”.`, () => {
      if (!rules.includes(removed))
        rules = [...rules.slice(0, index), removed, ...rules.slice(index)];
    });
  }
</script>

{#if rules.length === 0}
  <EmptyState>{empty}</EmptyState>
{:else}
  <ul class="rule-list">
    {#each rules as rule, index (index)}
      <li class="rule-row">
        <span class="rule-text">{rule}</span>
        <IconButton icon="trash" label="Remove the rule “{rule}”" onclick={() => remove(index)} />
      </li>
    {/each}
  </ul>
{/if}
<form
  class="rule-add"
  onsubmit={(event) => {
    event.preventDefault();
    add();
  }}
>
  <div class="rule-add-box">
    <TextField
      label={addLabel}
      labelHidden
      bind:value={draft}
      {placeholder}
      autocomplete="off"
      error={duplicate ? "That rule is already in the list." : error}
      oninput={() => {
        duplicate = false;
      }}
    />
  </div>
  <Button type="submit" icon="plus" disabled={draft.trim() === ""}>Add</Button>
</form>

<style>
  .rule-list {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .rule-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
    padding: var(--space-2) 0;
    border-top: 1px solid var(--color-line-soft);
  }

  .rule-row:first-child {
    border-top: 0;
  }

  .rule-text {
    min-width: 0;
    font-size: var(--font-size-sm);
    color: var(--color-text);
    overflow-wrap: anywhere;
  }

  .rule-add {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
  }

  .rule-add-box {
    flex: 1;
    min-width: 0;
  }
</style>
