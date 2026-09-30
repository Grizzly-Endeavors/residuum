<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";
  import { Icon, type IconName } from "../icons";
  import Spinner from "./Spinner.svelte";
  import { tooltipProvider } from "./tooltip";
  import type { ButtonSize, ButtonVariant } from "./types";

  interface Props extends Omit<HTMLButtonAttributes, "children" | "aria-label" | "aria-pressed"> {
    icon: IconName;
    /** The button's accessible name, and its tooltip unless `tooltip` says otherwise. */
    label: string;
    variant?: ButtonVariant;
    size?: ButtonSize;
    /** Set for a toggle button: the button reports `aria-pressed` and shows as selected. */
    pressed?: boolean;
    loading?: boolean;
    /** Tooltip text when it should differ from the label, or `false` for none. */
    tooltip?: string | false;
    element?: HTMLButtonElement;
  }

  let {
    icon,
    label,
    variant = "quiet",
    size = "md",
    pressed,
    loading = false,
    tooltip,
    type = "button",
    element = $bindable(),
    class: className,
    onclick,
    ...rest
  }: Props = $props();

  const provider = tooltipProvider();
  const tooltipText = $derived(tooltip === false ? null : (tooltip ?? label));
  const iconSize = $derived(size === "sm" ? 14 : 16);
</script>

<button
  bind:this={element}
  {...rest}
  {@attach provider && tooltipText !== null ? provider(tooltipText) : undefined}
  {type}
  class={["ui-icon-button", className]}
  data-variant={variant}
  data-size={size}
  aria-label={label}
  aria-pressed={pressed}
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
  {:else}
    <Icon name={icon} size={iconSize} />
  {/if}
</button>

<style>
  .ui-icon-button {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--corner-sm);
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);
  }

  .ui-icon-button[data-size="sm"] {
    width: 28px;
    height: 28px;
  }

  .ui-icon-button[data-variant="primary"] {
    background: var(--color-vein-dim);
    color: var(--color-on-accent);
  }

  .ui-icon-button[data-variant="secondary"] {
    background: var(--color-stone-3);
    color: var(--color-text);
  }

  .ui-icon-button[data-variant="quiet"] {
    background: transparent;
    color: var(--color-text-2);
  }

  .ui-icon-button[data-variant="danger"] {
    background: transparent;
    color: var(--color-err-text);
  }

  .ui-icon-button[aria-pressed="true"] {
    background: var(--color-vein-tint);
    color: var(--color-text);
  }

  .ui-icon-button:not(:disabled, [aria-disabled="true"]) {
    &:hover[data-variant="primary"] {
      background: var(--color-vein-hover);
    }

    &:hover:is([data-variant="secondary"], [aria-pressed="true"]) {
      background: var(--color-stone-4);
      color: var(--color-text);
    }

    &:hover[data-variant="quiet"]:not([aria-pressed="true"]) {
      background: var(--color-stone-3);
      color: var(--color-text);
    }

    &:hover[data-variant="danger"] {
      background: var(--color-err-tint);
    }

    &:active {
      transform: translateY(1px);
    }
  }

  .ui-icon-button:disabled {
    opacity: 0.4;
  }

  .ui-icon-button[aria-busy="true"] {
    cursor: progress;
  }

  @media (max-width: 760px) {
    .ui-icon-button,
    .ui-icon-button[data-size="sm"] {
      width: var(--layout-touch-target);
      height: var(--layout-touch-target);
    }
  }
</style>
