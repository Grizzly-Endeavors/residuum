<script lang="ts">
  import { Toggle } from "../../lib/ui";
  import type { FlagKey } from "./config-keys";
  import { fieldError, type SettingsScope } from "./sections";

  interface Props {
    scope: SettingsScope;
    field: FlagKey;
    label: string;
    hint?: string;
    disabled?: boolean;
  }

  let { scope, field, label, hint, disabled }: Props = $props();
</script>

<Toggle
  {label}
  {hint}
  {disabled}
  error={fieldError(scope, { kind: "config", field })}
  bind:checked={
    () => scope.config[field],
    (on) => {
      scope.config[field] = on;
    }
  }
/>
