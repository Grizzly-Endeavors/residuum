<script lang="ts">
  import type { Snippet } from "svelte";
  import { Icon } from "../icons";
  import type { FieldControl } from "./types";

  interface Props {
    label: string;
    /** Help under the control, linked to it as a description. */
    hint?: string;
    /** An inline error under the control; marks the control invalid and describes it. */
    error?: string;
    /** Keep the label for assistive technology only. */
    labelHidden?: boolean;
    /**
     * `label` for a control a `<label for>` can name (inputs, selects,
     * buttons); `span` for one named by `aria-labelledby` (radio groups, groups).
     */
    labelElement?: "label" | "span";
    /** Label above the control, label beside it with the control at the end, or both on one compact line. */
    layout?: "stack" | "row" | "inline";
    children: Snippet<[FieldControl]>;
  }

  let {
    label,
    hint,
    error,
    labelHidden = false,
    labelElement = "label",
    layout = "stack",
    children,
  }: Props = $props();

  const uid = $props.id();
  const hintId = `${uid}-hint`;
  const errorId = `${uid}-error`;

  const control = $derived<FieldControl>({
    id: `${uid}-control`,
    labelId: `${uid}-label`,
    describedBy:
      [hint ? hintId : null, error ? errorId : null].filter((id) => id !== null).join(" ") ||
      undefined,
    invalid: Boolean(error),
  });
</script>

<div class="ui-field" data-layout={layout}>
  <svelte:element
    this={labelElement}
    id={control.labelId}
    for={labelElement === "label" ? control.id : undefined}
    class={["ui-field-label", labelHidden && "ui-field-label-hidden"]}>{label}</svelte:element
  >
  <div class="ui-field-control">{@render children(control)}</div>
  {#if hint}
    <p id={hintId} class="ui-field-hint">{hint}</p>
  {/if}
  {#if error}
    <p id={errorId} class="ui-field-error">
      <Icon name="warning" size={14} />
      <span>{error}</span>
    </p>
  {/if}
</div>

<style>
  .ui-field {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    min-width: 0;
  }

  .ui-field-label {
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    color: var(--color-text);
  }

  .ui-field-label-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .ui-field-control {
    display: flex;
    min-width: 0;
  }

  .ui-field-hint {
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
    color: var(--color-text-3);
  }

  .ui-field-error {
    display: flex;
    align-items: flex-start;
    gap: var(--space-6);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
    color: var(--color-err-text);

    & > :global(svg) {
      margin-top: var(--space-2);
    }
  }

  .ui-field[data-layout="row"] {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-areas:
      "label control"
      "hint control"
      "error error";
    column-gap: var(--space-16);
    row-gap: var(--space-2);
    align-items: start;

    & > .ui-field-label {
      grid-area: label;
    }

    & > .ui-field-control {
      grid-area: control;
      align-self: center;
    }

    & > .ui-field-hint {
      grid-area: hint;
    }

    & > .ui-field-error {
      grid-area: error;
      margin-top: var(--space-4);
    }
  }

  .ui-field[data-layout="inline"] {
    flex-flow: row wrap;
    align-items: center;
    gap: var(--space-4) var(--space-8);

    & > .ui-field-label {
      font-weight: var(--font-weight-regular);
      color: var(--color-text-2);
    }

    & > :is(.ui-field-hint, .ui-field-error) {
      flex-basis: 100%;
    }
  }
</style>
