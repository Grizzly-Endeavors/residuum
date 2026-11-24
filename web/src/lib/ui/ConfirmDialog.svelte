<script lang="ts">
  import Button from "./Button.svelte";
  import Dialog from "./Dialog.svelte";
  import type { ConfirmTone } from "./types";

  interface Props {
    open: boolean;
    title: string;
    message?: string;
    items?: readonly string[];
    confirmLabel: string;
    cancelLabel?: string;
    tone?: ConfirmTone;
    onconfirm: () => void;
    oncancel?: () => void;
  }

  let {
    open = $bindable(false),
    title,
    message,
    items = [],
    confirmLabel,
    cancelLabel = "Cancel",
    tone = "default",
    onconfirm,
    oncancel,
  }: Props = $props();

  function cancel(): void {
    open = false;
    oncancel?.();
  }

  function confirm(): void {
    open = false;
    onconfirm();
  }
</script>

{#snippet affected()}
  <ul class="ui-confirm-items">
    {#each items as item, i (i)}
      <li>{item}</li>
    {/each}
  </ul>
{/snippet}

{#snippet choices()}
  <Button variant="quiet" data-confirm-cancel onclick={cancel}>{cancelLabel}</Button>
  <Button variant={tone === "danger" ? "danger" : "primary"} data-confirm-accept onclick={confirm}>
    {confirmLabel}
  </Button>
{/snippet}

<Dialog
  bind:open
  {title}
  description={message}
  size="sm"
  role="alertdialog"
  closeButton={false}
  initialFocus={tone === "danger" ? "[data-confirm-cancel]" : "[data-confirm-accept]"}
  onclose={() => oncancel?.()}
  children={items.length > 0 ? affected : undefined}
  actions={choices}
/>

<style>
  .ui-confirm-items {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding-left: var(--space-18);
    font-size: var(--font-size-sm);
    color: var(--color-text);

    & li::marker {
      color: var(--color-text-3);
    }
  }
</style>
