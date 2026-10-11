<script lang="ts">
  import type { Snippet } from "svelte";
  import IconButton from "./IconButton.svelte";
  import ModalLayer from "./ModalLayer.svelte";
  import type { DialogSize } from "./types";

  interface Props {
    open: boolean;
    title: string;
    /** A line under the title. It is the dialog's description for assistive technology. */
    description?: string;
    size?: DialogSize;
    /** Fill the screen at phone width, for long content and forms. */
    fullscreenOnPhone?: boolean;
    role?: "dialog" | "alertdialog";
    /** What takes focus on open: a selector's match, or false for the dialog itself. */
    initialFocus?: string | false;
    closeButton?: boolean;
    /** The dialog closed from the UI or by Back. */
    onclose?: () => void;
    children?: Snippet;
    /** Buttons along the bottom edge, the main action last. */
    actions?: Snippet;
  }

  let {
    open = $bindable(false),
    title,
    description,
    size = "md",
    fullscreenOnPhone = false,
    role = "dialog",
    initialFocus,
    closeButton = true,
    onclose,
    children,
    actions,
  }: Props = $props();

  const WIDTHS: Record<DialogSize, string> = { sm: "400px", md: "480px", lg: "640px" };
  const uid = $props.id();

  function close(): void {
    open = false;
    onclose?.();
  }
</script>

<ModalLayer
  {open}
  onclose={close}
  width={WIDTHS[size]}
  {fullscreenOnPhone}
  {role}
  labelledby="{uid}-title"
  describedby={description === undefined ? undefined : `${uid}-description`}
  {initialFocus}
>
  <header class="ui-dialog-head">
    <div class="ui-dialog-titles">
      <h2 id="{uid}-title" class="ui-dialog-title">{title}</h2>
      {#if description !== undefined}
        <p id="{uid}-description" class="ui-dialog-description">{description}</p>
      {/if}
    </div>
    {#if closeButton}
      <IconButton icon="close" label="Close" size="sm" data-overlay-close onclick={close} />
    {/if}
  </header>
  {#if children}
    <div class="ui-dialog-body">{@render children()}</div>
  {/if}
  {#if actions}
    <footer class="ui-dialog-actions">{@render actions()}</footer>
  {/if}
</ModalLayer>

<style>
  .ui-dialog-head {
    display: flex;
    flex: none;
    align-items: flex-start;
    gap: var(--space-12);
    padding: var(--space-20) var(--space-16) var(--space-4) var(--space-24);
  }

  .ui-dialog-titles {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
    padding-top: var(--space-2);
  }

  .ui-dialog-title {
    font-size: var(--font-size-heading);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
    overflow-wrap: anywhere;
  }

  .ui-dialog-description {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .ui-dialog-body {
    flex: 1 1 auto;
    min-height: 0;
    padding: var(--space-12) var(--space-24) var(--space-20);
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  .ui-dialog-actions {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-8);
    padding: 0 var(--space-24) var(--space-20);
  }

  .ui-dialog-head:has(+ .ui-dialog-actions) {
    padding-bottom: var(--space-20);
  }

  @media (max-width: 760px) {
    .ui-dialog-head {
      padding: var(--space-16) var(--space-10) var(--space-4) var(--space-16);
    }

    .ui-dialog-body {
      padding: var(--space-12) var(--space-16) var(--space-16);
    }

    .ui-dialog-actions {
      padding: 0 var(--space-16) var(--space-16);
    }
  }
</style>
