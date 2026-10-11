<script lang="ts">
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import { NumberField } from "../../lib/ui";
  import type { TextKey } from "./config-keys";
  import { fieldError, type SettingsScope } from "./sections";

  // A number in the scope's `config.toml` form, bound through
  // `lib/settings-bind.ts`. The form keeps it as text, so a blank box is an
  // unset key, which is read as its default. `fallback` shows that default in
  // the box while it's blank and ends the hint, with the unit, so every number
  // reads with what it counts; a section that words the default itself passes
  // `placeholder` alone.

  interface Props {
    scope: SettingsScope;
    field: TextKey;
    label: string;
    /** What the number does, in a sentence. */
    hint?: string;
    /** What the number counts, shown after the box: "minutes", "tokens". */
    unit?: string;
    /** What the agent uses while the box is blank, when that is a number. */
    fallback?: number;
    /** Shown while the box is blank, when blank means something other than a number. */
    placeholder?: string;
    min?: number;
    max?: number;
    step?: number;
    disabled?: boolean;
  }

  let { scope, field, label, hint, unit, fallback, placeholder, min, max, step, disabled }: Props =
    $props();

  const defaultNote = $derived(
    fallback === undefined
      ? undefined
      : `Default: ${fallback.toLocaleString("en-US")}${unit ? ` ${unit}` : ""}.`,
  );
  const help = $derived([hint, defaultNote].filter(Boolean).join(" ") || undefined);
</script>

<NumberField
  {label}
  {unit}
  {min}
  {max}
  {step}
  {disabled}
  hint={help}
  placeholder={placeholder ?? (fallback === undefined ? undefined : String(fallback))}
  error={fieldError(scope, { kind: "config", field })}
  bind:value={
    () => numberOfText(scope.config[field]),
    (next) => {
      scope.config[field] = textOfNumber(next);
    }
  }
/>
