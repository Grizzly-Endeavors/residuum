<script lang="ts">
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import { NumberField } from "../../lib/ui";
  import type { TextKey } from "./config-keys";
  import { fieldError, type SettingsScope } from "./sections";

  interface Props {
    scope: SettingsScope;
    field: TextKey;
    label: string;
    hint?: string;
    unit?: string;
    fallback?: number;
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
