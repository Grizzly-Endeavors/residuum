<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import { Icon, type IconName } from "../icons";
  import Spinner from "./Spinner.svelte";
  import type { ButtonSize, ButtonVariant } from "./types";

  interface Props extends Omit<HTMLButtonAttributes, "children"> {
    variant?: ButtonVariant;
    size?: ButtonSize;
    /** A leading icon. A spinner takes its place while loading. */
    icon?: IconName;
    /**
     * The action is in progress: the button shows a spinner and ignores
     * presses, but stays focusable so keyboard focus isn't dropped.
     */
    loading?: boolean;
    element?: HTMLButtonElement;
    children: Snippet;
  }

  let {
    variant = "secondary",
    size = "md",
    icon,
    loading = false,
    type = "button",
    element = $bindable(),
    class: className,
    onclick,
    children,
    ...rest
  }: Props = $props();

  const iconSize = $derived(size === "sm" ? 14 : 16);
</script>

<button
  bind:this={element}
  {...rest}
  {type}
  class={["ui-button", className]}
  data-variant={variant}
  data-size={size}
  aria-disabled={loading || undefined}
  aria-busy={loading || undefined}
  onclick={(event) => {
    if (loading) {
      event.preventDefault();
      return;
    }
    onclick?.(event);
  }}
>
  {#if loading}
    <Spinner size={iconSize - 3} />
  {:else if icon}
    <Icon name={icon} size={iconSize} />
  {/if}
  <span class="ui-button-label">{@render children()}</span>
</button>

<style>
  .ui-button {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    gap: var(--space-6);
    height: 32px;
    padding: 0 var(--space-12);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    line-height: 1;
    white-space: nowrap;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);
  }

  .ui-button[data-size="sm"] {
    height: 28px;
    padding: 0 var(--space-10);
  }

  .ui-button-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .ui-button[data-variant="primary"] {
    background: var(--color-vein-dim);
    color: var(--color-on-accent);
  }

  .ui-button[data-variant="secondary"] {
    background: var(--color-stone-3);
    color: var(--color-text);
  }

  .ui-button[data-variant="quiet"] {
    background: transparent;
    color: var(--color-text-2);
  }

  .ui-button[data-variant="danger"] {
    background: transparent;
    color: var(--color-err-text);
  }

  .ui-button:not(:disabled, [aria-disabled="true"]) {
    &:hover[data-variant="primary"] {
      background: var(--color-vein-hover);
    }

    &:hover[data-variant="secondary"] {
      background: var(--color-stone-4);
    }

    /* A floating layer sets --ui-quiet-hover: stone-3 wouldn't show on its stone-3 card. */
    &:hover[data-variant="quiet"] {
      background: var(--ui-quiet-hover, var(--color-stone-3));
      color: var(--color-text);
    }

    /* err-text doesn't sit on a tint, so the label turns to text over the wash. */
    &:hover[data-variant="danger"] {
      background: var(--color-err-tint);
      color: var(--color-text);
    }

    &:active {
      transform: translateY(1px);
    }
  }

  .ui-button:disabled {
    opacity: 0.4;
  }

  .ui-button[aria-busy="true"] {
    cursor: progress;
  }

  @media (max-width: 760px) {
    .ui-button,
    .ui-button[data-size="sm"] {
      height: var(--layout-touch-target);
    }
  }
</style>
