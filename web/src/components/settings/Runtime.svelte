<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">General</div>
    <div class="settings-field">
      <label>Name</label>
      <input type="text" bind:value={fields.name} placeholder="What the agent calls you" />
    </div>
    <div class="settings-field">
      <label>Timezone</label>
      <input type="text" bind:value={fields.timezone} placeholder="e.g. America/New_York" />
    </div>
    <div class="settings-field">
      <label>Workspace Directory</label>
      <input type="text" bind:value={fields.workspace_dir} placeholder="Default: ~/.residuum/workspace" />
    </div>
    <div class="settings-field">
      <label>Timeout (seconds)</label>
      <input type="number" bind:value={fields.timeout_secs} placeholder="Default: 120" />
    </div>
    <div class="settings-field">
      <label>Max Tokens</label>
      <input type="number" bind:value={fields.max_tokens} placeholder="Default: 8192" />
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Gateway</div>
    <div class="settings-field">
      <label>Bind Address</label>
      <input type="text" bind:value={fields.gateway_bind} placeholder="Default: 127.0.0.1" />
    </div>
    <div class="settings-field">
      <label>Port</label>
      <input type="number" bind:value={fields.gateway_port} placeholder="Default: 7700" />
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Pulse & Background</div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.pulse_enabled} />
          <span class="toggle-slider"></span>
        </span>
        Pulse Enabled
      </label>
    </div>
    <div class="settings-field">
      <label>Max Concurrent Background Tasks</label>
      <input type="number" bind:value={fields.bg_max_concurrent} placeholder="Default: 3" />
    </div>
    <div class="settings-field">
      <label>Transcript Retention (days)</label>
      <input type="number" bind:value={fields.bg_transcript_retention_days} placeholder="Default: 7" />
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Retry</div>
    <div class="settings-field">
      <label>Max Retries</label>
      <input type="number" bind:value={fields.retry_max_retries} placeholder="Default: 3" />
    </div>
    <div class="settings-field">
      <label>Initial Delay (ms)</label>
      <input type="number" bind:value={fields.retry_initial_delay_ms} placeholder="Default: 1000" />
    </div>
    <div class="settings-field">
      <label>Max Delay (ms)</label>
      <input type="number" bind:value={fields.retry_max_delay_ms} placeholder="Default: 30000" />
    </div>
    <div class="settings-field">
      <label>Backoff Multiplier</label>
      <input type="number" step="0.1" bind:value={fields.retry_backoff_multiplier} placeholder="Default: 2.0" />
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Agent Abilities</div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.agent_modify_mcp} />
          <span class="toggle-slider"></span>
        </span>
        Allow MCP Modifications
      </label>
    </div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.agent_modify_channels} />
          <span class="toggle-slider"></span>
        </span>
        Allow Channel Modifications
      </label>
    </div>
  </div>
</div>
