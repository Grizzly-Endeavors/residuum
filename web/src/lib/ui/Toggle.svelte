<script lang="ts">
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Field from "./Field.svelte";
  import Spinner from "./Spinner.svelte";

  interface Props extends Omit<
    HTMLButtonAttributes,
    "children" | "id" | "role" | "onchange" | "aria-checked"
  > {
    label: string;
    checked?: boolean;
    hint?: string;
    error?: string;
    /**
     * `row` puts the label and hint on the left and the switch at the end, as
     * in a settings group; `inline` sets a compact label just before it.
     */
    layout?: "row" | "inline";
    /** The change is being applied: a spinner shows and presses are ignored. */
    loading?: boolean;
    onchange?: (checked: boolean) => void;
    element?: HTMLButtonElement;
  }

  let {
    label,
    checked = $bindable(false),
    hint,
    error,
    layout = "row",
    loading = false,
    onchange,
    element = $bindable(),
    ...rest
  }: Props = $props();

  function toggle(): void {
    if (loading) return;
    checked = !checked;
    onchange?.(checked);
  }
</script>

<Field {label} {hint} {error} {layout}>
  {#snippet children(control)}
    <span class="ui-toggle">
      {#if loading}
        <Spinner size={12} />
      {/if}
      <button
        bind:this={element}
        {...rest}
        id={control.id}
        type="button"
        role="switch"
        class="ui-switch"
        aria-checked={checked}
        aria-describedby={control.describedBy}
        aria-invalid={control.invalid || undefined}
        aria-disabled={loading || undefined}
        aria-busy={loading || undefined}
        onclick={toggle}
      >
        <span class="ui-switch-thumb"></span>
      </button>
    </span>
  {/snippet}
</Field>

<style>
  .ui-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-text-3);
  }

  .ui-switch {
    position: relative;
    flex: none;
    width: 36px;
    height: 20px;
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-pill);
    background: transparent;
    transition:
      background-color var(--duration-base) var(--ease-out),
      border-color var(--duration-base) var(--ease-out);
  }

  .ui-switch-thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--color-text-2);
    transition:
      transform var(--duration-base) var(--ease-out),
      background-color var(--duration-base) var(--ease-out);
  }

  .ui-switch[aria-checked="true"] {
    border-color: var(--color-vein);
    background: var(--color-vein-dim);

    & .ui-switch-thumb {
      background: var(--color-on-accent);
      transform: translateX(16px);
    }
  }

  .ui-switch:not(:disabled, [aria-disabled="true"]):hover {
    border-color: var(--color-text-2);

    &[aria-checked="true"] {
      border-color: var(--color-vein-bright);
      background: var(--color-vein-hover);
    }

    & .ui-switch-thumb {
      background: var(--color-text);
    }

    &[aria-checked="true"] .ui-switch-thumb {
      background: var(--color-on-accent);
    }
  }

  .ui-switch:disabled {
    opacity: 0.4;
  }

  .ui-switch[aria-busy="true"] {
    cursor: progress;
  }

  @media (max-width: 760px) {
    /* The track stays small; its hit area grows to the touch target. */
    .ui-switch::before {
      content: "";
      position: absolute;
      inset: -13px -5px;
    }
  }
</style>
