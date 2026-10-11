<script lang="ts">
  import type { Snippet } from "svelte";
  import ModalLayer from "./ModalLayer.svelte";

  // A bottom sheet, for choices and short forms on phones. Swiping it down,
  // Esc, the scrim and Back close it.

  interface Props {
    open: boolean;
    /** A visible heading, which names the sheet. */
    title?: string;
    /** The sheet's name when it has no title. */
    label?: string;
    description?: string;
    initialFocus?: string | false;
    onclose?: () => void;
    children: Snippet;
    actions?: Snippet;
  }

  let {
    open = $bindable(false),
    title,
    label,
    description,
    initialFocus,
    onclose,
    children,
    actions,
  }: Props = $props();

  const uid = $props.id();

  function close(): void {
    open = false;
    onclose?.();
  }
</script>

<ModalLayer
  {open}
  frame="bottom"
  onclose={close}
  label={title === undefined ? label : undefined}
  labelledby={title === undefined ? undefined : `${uid}-title`}
  describedby={description === undefined ? undefined : `${uid}-description`}
  {initialFocus}
>
  <div class="ui-sheet-grab" aria-hidden="true"></div>
  {#if title !== undefined || description !== undefined}
    <header class="ui-sheet-head">
      {#if title !== undefined}
        <h2 id="{uid}-title" class="ui-sheet-title">{title}</h2>
      {/if}
      {#if description !== undefined}
        <p id="{uid}-description" class="ui-sheet-description">{description}</p>
      {/if}
    </header>
  {/if}
  <div class="ui-sheet-body">{@render children()}</div>
  {#if actions}
    <footer class="ui-sheet-actions">{@render actions()}</footer>
  {/if}
</ModalLayer>

<style>
  .ui-sheet-grab {
    flex: none;
    width: 36px;
    height: 4px;
    margin: var(--space-8) auto var(--space-6);
    border-radius: var(--corner-pill);
    background: var(--color-stone-4);
  }

  .ui-sheet-head {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-6) var(--space-16) var(--space-4);
  }

  .ui-sheet-title {
    font-size: var(--font-size-heading);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .ui-sheet-description {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .ui-sheet-body {
    flex: 1 1 auto;
    min-height: 0;
    padding: var(--space-8) var(--space-16) var(--space-16);
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  .ui-sheet-actions {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-8);
    padding: 0 var(--space-16) var(--space-16);
  }
</style>
