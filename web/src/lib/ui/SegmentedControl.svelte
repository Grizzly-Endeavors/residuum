<script lang="ts" generics="T extends string">
  import Field from "./Field.svelte";
  import type { Choice } from "./types";

  interface Props {
    label: string;
    value: T;
    options: readonly Choice<T>[];
    hint?: string;
    error?: string;
    labelHidden?: boolean;
    disabled?: boolean;
    onchange?: (value: T) => void;
  }

  let {
    label,
    value = $bindable(),
    options,
    hint,
    error,
    labelHidden = false,
    disabled = false,
    onchange,
  }: Props = $props();

  // One tab stop for the group: the chosen option, or the first one that can
  // be chosen when none is.
  const tabStop = $derived(
    options.find((option) => option.value === value && !option.disabled)?.value ??
      options.find((option) => !option.disabled)?.value,
  );

  function choose(option: Choice<T>): void {
    if (disabled || option.disabled || option.value === value) return;
    value = option.value;
    onchange?.(option.value);
  }

  const STEPS: Readonly<Record<string, "next" | "previous" | "first" | "last">> = {
    ArrowRight: "next",
    ArrowDown: "next",
    ArrowLeft: "previous",
    ArrowUp: "previous",
    Home: "first",
    End: "last",
  };

  // Arrow keys move through the enabled options, wrapping at the ends, and
  // choose the one they land on, as a radio group does.
  function onkeydown(event: KeyboardEvent & { currentTarget: HTMLElement }): void {
    const step = STEPS[event.key];
    if (step === undefined || disabled) return;
    const enabled = options.filter((option) => !option.disabled);
    if (enabled.length === 0) return;
    event.preventDefault();
    const current = enabled.findIndex((option) => option.value === tabStop);
    let index: number;
    if (step === "first") index = 0;
    else if (step === "last") index = enabled.length - 1;
    else {
      const offset = step === "next" ? 1 : -1;
      index = (current + offset + enabled.length) % enabled.length;
    }
    const target = enabled[index];
    if (target === undefined) return;
    choose(target);
    const position = options.indexOf(target);
    event.currentTarget.parentElement
      ?.querySelectorAll<HTMLElement>('[role="radio"]')
      [position]?.focus();
  }
</script>

<Field {label} {hint} {error} {labelHidden} labelElement="span">
  {#snippet children(control)}
    <div
      class="ui-segmented"
      role="radiogroup"
      aria-labelledby={control.labelId}
      aria-describedby={control.describedBy}
      aria-invalid={control.invalid || undefined}
      aria-disabled={disabled || undefined}
    >
      {#each options as option (option.value)}
        <button
          type="button"
          role="radio"
          class="ui-segment"
          aria-checked={option.value === value}
          tabindex={option.value === tabStop ? 0 : -1}
          disabled={disabled || option.disabled}
          {onkeydown}
          onclick={() => {
            choose(option);
          }}
        >
          {option.label}
        </button>
      {/each}
    </div>
  {/snippet}
</Field>

<style>
  .ui-segmented {
    display: inline-flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    padding: 3px;
    border-radius: var(--corner-md);
    background: var(--color-stone-3);
  }

  .ui-segment {
    height: 28px;
    padding: 0 var(--space-12);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);
  }

  .ui-segment:not(:disabled):hover {
    color: var(--color-text);
  }

  .ui-segment[aria-checked="true"] {
    background: var(--color-stone-0);
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .ui-segment:disabled {
    opacity: 0.4;
  }

  @media (max-width: 760px) {
    .ui-segment {
      height: var(--layout-touch-target);
      padding: 0 var(--space-16);
    }
  }
</style>
