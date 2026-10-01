<script lang="ts" generics="T extends { name: string }">
  import type { Snippet } from "svelte";

  // The rows of a saved list in Saved keys and Agent-to-agent: each is a
  // name, what to know about it beside and under the name, and the action
  // that removes it at the end. The list never shows a value.

  interface Props {
    /** Names the list for assistive technology. */
    label: string;
    items: readonly T[];
    /** Beside the name: an environment variable, a badge, when it was made. */
    mark?: Snippet<[T]>;
    /** Under the name: what it is for. An empty line shows nothing. */
    note?: (item: T) => string;
    /** The row's action, an icon button. */
    action: Snippet<[T]>;
  }

  let { label, items, mark, note, action }: Props = $props();
</script>

<ul class="key-list" aria-label={label}>
  {#each items as item (item.name)}
    {@const line = note?.(item) ?? ""}
    <li class="key-row">
      <div class="key-text">
        <div class="key-head">
          <span class="key-name">{item.name}</span>
          {@render mark?.(item)}
        </div>
        {#if line !== ""}
          <p class="key-note">{line}</p>
        {/if}
      </div>
      {@render action(item)}
    </li>
  {/each}
</ul>

<style>
  .key-list {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .key-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
    padding: var(--space-8) 0;
    border-top: 1px solid var(--color-line-soft);

    &:first-child {
      border-top: 0;
    }
  }

  .key-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .key-head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-2) var(--space-10);
  }

  .key-name {
    font-family: var(--font-code);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    overflow-wrap: anywhere;
  }

  .key-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    overflow-wrap: anywhere;
  }
</style>
