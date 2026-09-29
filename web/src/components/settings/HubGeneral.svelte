<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable(), simple = false }: { fields: ConfigFields; simple?: boolean } =
    $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Timezone</div>
    <div class="settings-field">
      <label for="hub-timezone">Timezone</label>
      <input
        id="hub-timezone"
        type="text"
        bind:value={fields.timezone}
        placeholder="e.g. America/New_York"
      />
      <div class="field-hint">Used for every agent's schedules and timestamps.</div>
    </div>
  </div>

  {#if !simple}
    <div class="settings-group">
      <div class="settings-group-label">Gateway</div>
      <div class="settings-field">
        <label for="hub-gateway-bind">Bind Address</label>
        <input
          id="hub-gateway-bind"
          type="text"
          bind:value={fields.gateway_bind}
          placeholder="Default: 127.0.0.1"
        />
      </div>
      <div class="settings-field">
        <label for="hub-gateway-port">Port</label>
        <input
          id="hub-gateway-port"
          type="number"
          bind:value={fields.gateway_port}
          placeholder="Default: 7700"
        />
      </div>
    </div>
  {/if}
</div>
