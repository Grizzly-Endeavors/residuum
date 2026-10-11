<script lang="ts">
  import type { HTMLInputAttributes } from "svelte/elements";
  import Field from "./Field.svelte";
  import Input from "./Input.svelte";

  interface Props extends Omit<HTMLInputAttributes, "value" | "type" | "children" | "id"> {
    label: string;
    /** `null` while the box is empty. */
    value?: number | null;
    hint?: string;
    error?: string;
    labelHidden?: boolean;
    /** What the number counts, shown after the box: "messages", "seconds". */
    unit?: string;
    element?: HTMLInputElement | HTMLTextAreaElement;
  }

  let {
    label,
    value = $bindable(null),
    hint,
    error,
    labelHidden = false,
    unit,
    element = $bindable(),
    ...rest
  }: Props = $props();

  const uid = $props.id();
  const unitId = `${uid}-unit`;
</script>

<Field {label} {hint} {error} {labelHidden}>
  {#snippet children(control)}
    <span class="ui-number-field">
      <Input
        bind:value
        bind:element
        inputmode="decimal"
        {...rest}
        type="number"
        short
        id={control.id}
        invalid={control.invalid}
        aria-describedby={[unit ? unitId : null, control.describedBy]
          .filter((id) => id != null)
          .join(" ") || undefined}
      />
      {#if unit}
        <span id={unitId} class="ui-number-field-unit">{unit}</span>
      {/if}
    </span>
  {/snippet}
</Field>

<style>
  .ui-number-field {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
  }

  .ui-number-field-unit {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }
</style>
