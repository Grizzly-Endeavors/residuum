<script lang="ts">
  import { Icon, type IconName } from "../icons";
  import { toast, type Toast, type ToastKind } from "../toast.svelte";
  import IconButton from "./IconButton.svelte";
  import { portal } from "./overlay/host";

  // The toast store's toasts, along the bottom edge. The region lives in the
  // overlay host above every layer and never joins the overlay stack, so an
  // Undo stays within reach while a dialog is open. The store sets the
  // timings: info and success leave after 4 seconds, or 10 with an action,
  // and errors stay until dismissed. The shell mounts one region.

  const ICONS: Readonly<Record<ToastKind, IconName>> = {
    info: "info",
    success: "check",
    error: "warning",
  };

  const toasts = $derived([...toast.toasts.values()]);
  const errors = $derived(toasts.filter((item) => item.kind === "error"));
  const others = $derived(toasts.filter((item) => item.kind !== "error"));
</script>

{#snippet card(item: Toast)}
  <div class="ui-toast" data-kind={item.kind}>
    <span class="ui-toast-icon"><Icon name={ICONS[item.kind]} size={16} /></span>
    <p class="ui-toast-message">{item.message}</p>
    {#if item.action}
      <button type="button" class="ui-toast-action" onclick={() => toast.runAction(item.id)}>
        {item.action.label}
      </button>
    {/if}
    <!-- Its tooltip would sit under the toasts. -->
    <IconButton
      icon="close"
      label="Dismiss notification"
      size="sm"
      tooltip={false}
      onclick={() => toast.dismiss(item.id)}
    />
  </div>
{/snippet}

<!-- The region moves into the overlay host; these hold its place in the block. -->
<template></template>
<div class="ui-toasts" {@attach portal}>
  <!-- Errors interrupt; everything else is read at the next pause. Each new toast is read alone. -->
  <div class="ui-toast-group" role="alert" aria-atomic="false">
    {#each errors as item (item.id)}
      {@render card(item)}
    {/each}
  </div>
  <div class="ui-toast-group" role="status" aria-atomic="false">
    {#each others as item (item.id)}
      {@render card(item)}
    {/each}
  </div>
</div>
<template></template>

<style>
  .ui-toasts {
    position: fixed;
    bottom: var(--space-24);
    left: 50%;
    z-index: var(--z-toast);
    display: flex;
    flex-direction: column;
    width: min(460px, calc(100vw - 2 * var(--space-16)));
    transform: translateX(-50%);
    pointer-events: none;
  }

  .ui-toast-group {
    display: flex;
    flex-direction: column;
  }

  .ui-toast {
    /* Quiet buttons hover darker on the stone-4 card. */
    --ui-quiet-hover: var(--color-stone-3);

    display: flex;
    align-items: flex-start;
    gap: var(--space-10);
    /* Spaced from above, so either group can be empty; the region hangs from the bottom. */
    margin-top: var(--space-8);
    padding: var(--space-6) var(--space-6) var(--space-6) var(--space-14);
    border-radius: var(--corner-lg);
    background: var(--color-stone-4);
    box-shadow: var(--shadow-float);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    pointer-events: auto;
    animation: ui-toast-in var(--duration-base) var(--ease-out);
  }

  /* Icon and buttons line up with the message's first line when it wraps. */
  .ui-toast-icon {
    display: grid;
    flex: none;
    place-items: center;
    height: 28px;
    color: var(--color-text-2);

    [data-kind="success"] > & {
      color: var(--color-moss-text);
    }

    [data-kind="error"] > & {
      color: var(--color-err-text);
    }
  }

  .ui-toast-message {
    flex: 1;
    min-width: 0;
    padding: var(--space-4) 0;
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
  }

  /* On stone-4 only a tint carries the accent: vein-bright on vein-tint. */
  .ui-toast-action {
    flex: none;
    height: 28px;
    padding: 0 var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-vein-tint);
    color: var(--color-vein-bright);
    font-weight: var(--font-weight-medium);
    transition: color var(--duration-fast) var(--ease-out);

    &:hover {
      color: var(--color-text);
    }

    &:active {
      transform: translateY(1px);
    }
  }

  /* Clear of the bottom bar and the composer above it. */
  @media (max-width: 760px) {
    .ui-toasts {
      bottom: calc(var(--layout-bottom-bar-offset) + 96px);
    }

    .ui-toast-action {
      height: var(--layout-touch-target);
    }
  }

  @keyframes ui-toast-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
