<script lang="ts">
  import type { HTMLInputAttributes, HTMLTextareaAttributes } from "svelte/elements";

  // The text box every field draws: text, number and password inputs, and
  // multi-line text. Fields wrap it with a label, hint and error.
  interface Props extends Omit<HTMLInputAttributes, "value" | "children"> {
    value?: string | number | null;
    invalid?: boolean;
    /** Code, paths and ids: JetBrains Mono. */
    code?: boolean;
    /** Sized for a number or another value a few characters long. */
    short?: boolean;
    multiline?: boolean;
    rows?: number;
    element?: HTMLInputElement | HTMLTextAreaElement;
  }

  let {
    value = $bindable(),
    invalid = false,
    code = false,
    short = false,
    multiline = false,
    rows = 3,
    element = $bindable(),
    class: className,
    ...rest
  }: Props = $props();

  const classes = $derived(["ui-input", code && "ui-input-code", className]);
</script>

{#if multiline}
  <textarea
    bind:this={element}
    bind:value
    {...rest as HTMLTextareaAttributes}
    {rows}
    class={classes}
    data-short={short || undefined}
    aria-invalid={invalid || undefined}
  ></textarea>
{:else}
  <input
    bind:this={element}
    bind:value
    {...rest}
    class={classes}
    data-short={short || undefined}
    aria-invalid={invalid || undefined}
  />
{/if}

<style>
  .ui-input {
    width: 100%;
    min-width: 0;
    height: 36px;
    padding: 0 var(--space-12);
    border: 1px solid var(--color-control-border);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    color: var(--color-text);
    font-size: var(--font-size-ui);
    transition:
      border-color var(--duration-fast) var(--ease-out),
      box-shadow var(--duration-fast) var(--ease-out);
  }

  textarea.ui-input {
    height: auto;
    padding: var(--space-8) var(--space-12);
    line-height: var(--line-height-ui);
    resize: vertical;
  }

  .ui-input-code {
    font-family: var(--font-code);
    font-size: var(--font-size-sm);
  }

  .ui-input[data-short] {
    width: 120px;
  }

  /* The focus ring is the boundary itself, doubled in weight and haloed. */
  .ui-input:focus-visible {
    outline: none;
    border-color: var(--color-vein);
    box-shadow:
      inset 0 0 0 1px var(--color-vein),
      0 0 0 3px var(--color-vein-faint);
  }

  .ui-input[aria-invalid="true"] {
    border-color: var(--color-err);

    &:focus-visible {
      box-shadow:
        inset 0 0 0 1px var(--color-err),
        0 0 0 3px var(--color-err-tint);
    }
  }

  .ui-input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  @media (max-width: 760px) {
    .ui-input:not(textarea) {
      height: var(--layout-touch-target);
    }

    .ui-input,
    .ui-input-code {
      font-size: var(--font-size-field-phone);
    }
  }
</style>
