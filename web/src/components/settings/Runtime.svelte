<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">General</div>
    <div class="settings-field">
      <label for="rt-name">Name</label>
      <input
        id="rt-name"
        type="text"
        bind:value={fields.name}
        placeholder="What the agent calls you"
      />
    </div>
    <div class="settings-field">
      <label for="rt-timezone">Timezone</label>
      <input
        id="rt-timezone"
        type="text"
        bind:value={fields.timezone}
        placeholder="e.g. America/New_York"
      />
    </div>
    <div class="settings-field">
      <label for="rt-workspace-dir">Workspace Directory</label>
      <input
        id="rt-workspace-dir"
        type="text"
        bind:value={fields.workspace_dir}
        placeholder="Default: ~/.residuum/workspace"
      />
    </div>
    <div class="settings-field">
      <label for="rt-timeout">Timeout (seconds)</label>
      <input
        id="rt-timeout"
        type="number"
        bind:value={fields.timeout_secs}
        placeholder="Default: 120"
      />
    </div>
    <div class="settings-field">
      <label for="rt-max-tokens">Max Tokens</label>
      <input
        id="rt-max-tokens"
        type="number"
        bind:value={fields.max_tokens}
        placeholder="Default: 8192"
      />
    </div>
    <div class="settings-field">
      <label for="rt-temperature">Temperature</label>
      <input
        id="rt-temperature"
        type="number"
        step="0.1"
        min="0"
        max="2"
        bind:value={fields.temperature}
        placeholder="Provider default"
      />
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Gateway</div>
    <div class="settings-field">
      <label for="rt-gateway-bind">Bind Address</label>
      <input
        id="rt-gateway-bind"
        type="text"
        bind:value={fields.gateway_bind}
        placeholder="Default: 127.0.0.1"
      />
    </div>
    <div class="settings-field">
      <label for="rt-gateway-port">Port</label>
      <input
        id="rt-gateway-port"
        type="number"
        bind:value={fields.gateway_port}
        placeholder="Default: 7700"
      />
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
      <label for="rt-bg-max-concurrent">Max Concurrent Background Tasks</label>
      <input
        id="rt-bg-max-concurrent"
        type="number"
        bind:value={fields.bg_max_concurrent}
        placeholder="Default: 3"
      />
    </div>
    <div class="settings-field">
      <label for="rt-bg-transcript-retention">Transcript Retention (days)</label>
      <input
        id="rt-bg-transcript-retention"
        type="number"
        bind:value={fields.bg_transcript_retention_days}
        placeholder="Default: 7"
      />
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Retry</div>
    <div class="settings-field">
      <label for="rt-retry-max-retries">Max Retries</label>
      <input
        id="rt-retry-max-retries"
        type="number"
        bind:value={fields.retry_max_retries}
        placeholder="Default: 3"
      />
    </div>
    <div class="settings-field">
      <label for="rt-retry-initial-delay">Initial Delay (ms)</label>
      <input
        id="rt-retry-initial-delay"
        type="number"
        bind:value={fields.retry_initial_delay_ms}
        placeholder="Default: 1000"
      />
    </div>
    <div class="settings-field">
      <label for="rt-retry-max-delay">Max Delay (ms)</label>
      <input
        id="rt-retry-max-delay"
        type="number"
        bind:value={fields.retry_max_delay_ms}
        placeholder="Default: 30000"
      />
    </div>
    <div class="settings-field">
      <label for="rt-retry-backoff">Backoff Multiplier</label>
      <input
        id="rt-retry-backoff"
        type="number"
        step="0.1"
        bind:value={fields.retry_backoff_multiplier}
        placeholder="Default: 2.0"
      />
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

  <div class="settings-group">
    <div class="settings-group-label">Idle</div>
    <div class="settings-field">
      <label for="rt-idle-timeout">Timeout (minutes)</label>
      <input
        id="rt-idle-timeout"
        type="number"
        bind:value={fields.idle_timeout_minutes}
        placeholder="Default: 30 (0 = disabled)"
      />
    </div>
    <div class="settings-field">
      <label for="rt-idle-channel">Idle Channel</label>
      <select id="rt-idle-channel" bind:value={fields.idle_channel}>
        <option value="">Keep current</option>
        <option value="websocket">WebSocket</option>
        <option value="telegram">Telegram</option>
        <option value="discord">Discord</option>
      </select>
    </div>
  </div>
</div>
