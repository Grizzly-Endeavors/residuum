<script lang="ts">
  import { SelectField, Toggle } from "../../lib/ui";
  import { fieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Diagnostics: how much the log records, and what leaves this machine in
  // traces and bug reports. All three are staged and saved with the rest of
  // the install-wide settings; saving rewrites `[tracing]` in the hub's
  // config, which the hub applies at once and keeps across restarts.

  let { scope, section }: AllSectionProps = $props();

  const LOG_DETAIL = [
    { value: "", label: "Default (debug)" },
    { value: "info", label: "Info" },
    { value: "debug", label: "Debug" },
    { value: "trace", label: "Trace" },
  ];
</script>

<SettingsSection
  {scope}
  {section}
  title="Diagnostics"
  lede="Logs, and what Residuum sends to the developer when something goes wrong."
>
  <SettingsGroup title="Logs">
    <SelectField
      label="Log detail"
      bind:value={scope.config.tracing_log_level}
      options={LOG_DETAIL}
      hint="How much the log files record. More detail uses more disk. The RUST_LOG environment variable overrides this when it is set."
      error={fieldError(scope, { kind: "config", field: "tracing_log_level" })}
    />
  </SettingsGroup>

  <SettingsGroup title="Traces and bug reports">
    <Toggle
      label="Redact content in trace exports"
      bind:checked={scope.config.tracing_sanitize_content}
      hint="Removes message text and tool output from traces before they leave this machine."
      error={fieldError(scope, { kind: "config", field: "tracing_sanitize_content" })}
    />
    <Toggle
      label="Report errors automatically"
      bind:checked={scope.config.tracing_auto_error_reporting}
      hint="When an agent's turn fails unexpectedly, send the developer a bug report with recent trace data, at most five an hour. Off by default."
      error={fieldError(scope, { kind: "config", field: "tracing_auto_error_reporting" })}
    />
  </SettingsGroup>
</SettingsSection>
