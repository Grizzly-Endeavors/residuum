<script lang="ts">
  import type { HTMLSelectAttributes } from "svelte/elements";
  import { Icon } from "../icons";
  import Field from "./Field.svelte";
  import Spinner from "./Spinner.svelte";
  import type { Choice } from "./types";

  interface Props extends Omit<HTMLSelectAttributes, "value" | "children" | "id"> {
    label: string;
    value?: string;
    options: readonly Choice[];
    /** Shown while nothing is chosen (value ""); it can't be chosen back. */
    placeholder?: string;
    hint?: string;
    error?: string;
    labelHidden?: boolean;
    /** The options are still coming: the select is disabled and says so. */
    loading?: boolean;
    element?: HTMLSelectElement;
  }

  let {
    label,
    value = $bindable(""),
    options,
    placeholder,
    hint,
    error,
    labelHidden = false,
    loading = false,
    disabled = false,
    element = $bindable(),
    ...rest
  }: Props = $props();
</script>

<Field {label} {hint} {error} {labelHidden}>
  {#snippet children(control)}
    <span class="ui-select">
      <select
        bind:this={element}
        bind:value
        {...rest}
        id={control.id}
        class="ui-select-box"
        disabled={(disabled ?? false) || loading}
        aria-busy={loading || undefined}
        aria-invalid={control.invalid || undefined}
        aria-describedby={control.describedBy}
      >
        {#if loading}
          <option {value}>Loading…</option>
        {:else}
          {#if placeholder !== undefined}
            <option value="" disabled>{placeholder}</option>
          {/if}
          {#each options as option (option.value)}
            <option value={option.value} disabled={option.disabled}>{option.label}</option>
          {/each}
        {/if}
      </select>
      <span class="ui-select-mark" aria-hidden="true">
        {#if loading}
          <Spinner size={12} />
        {:else}
          <Icon name="chevron-down" size={14} />
        {/if}
      </span>
    </span>
  {/snippet}
</Field>

<style>
  .ui-select {
    position: relative;
    display: flex;
    width: 100%;
    min-width: 0;
  }

  .ui-select-box {
    width: 100%;
    min-width: 0;
    height: 36px;
    padding: 0 var(--space-32) 0 var(--space-12);
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    color: var(--color-text);
    font-size: var(--font-size-ui);
    text-overflow: ellipsis;
    appearance: none;
    cursor: pointer;
    transition:
      border-color var(--duration-fast) var(--ease-out),
      box-shadow var(--duration-fast) var(--ease-out);
  }

  .ui-select-box:focus-visible {
    outline: none;
    border-color: var(--color-vein);
    box-shadow:
      inset 0 0 0 1px var(--color-vein),
      0 0 0 3px var(--color-vein-faint);
  }

  .ui-select-box[aria-invalid="true"] {
    border-color: var(--color-err);

    &:focus-visible {
      box-shadow:
        inset 0 0 0 1px var(--color-err),
        0 0 0 3px var(--color-err-tint);
    }
  }

  .ui-select-box:disabled {
    cursor: not-allowed;
  }

  .ui-select-box:disabled:not([aria-busy="true"]) {
    opacity: 0.5;
  }

  .ui-select-mark {
    position: absolute;
    top: 50%;
    right: var(--space-12);
    display: grid;
    color: var(--color-text-2);
    transform: translateY(-50%);
    pointer-events: none;
  }

  @media (max-width: 760px) {
    .ui-select-box {
      height: var(--layout-touch-target);
      font-size: var(--font-size-field-phone);
    }
  }
</style>
