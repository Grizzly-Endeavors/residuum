<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Tracing</div>
    <div class="settings-field">
      <label for="tracing-log-level">Log detail</label>
      <select id="tracing-log-level" bind:value={fields.tracing_log_level}>
        <option value="">Default (debug)</option>
        <option value="info">Info</option>
        <option value="debug">Debug</option>
        <option value="trace">Trace</option>
      </select>
      <div class="field-hint">How much the log files record. More detail uses more disk.</div>
    </div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.tracing_sanitize_content} />
          <span class="toggle-slider"></span>
        </span>
        Redact content in trace exports
      </label>
      <div class="field-hint">
        Removes message text and tool output from traces before they leave this machine.
      </div>
    </div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.tracing_auto_error_reporting} />
          <span class="toggle-slider"></span>
        </span>
        Report errors automatically
      </label>
      <div class="field-hint">
        When an agent's turn fails unexpectedly, send a redacted report to the developer. Off by
        default.
      </div>
    </div>
  </div>
</div>
