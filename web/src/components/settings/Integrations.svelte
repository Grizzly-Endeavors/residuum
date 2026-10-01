<script lang="ts">
  import { onMount } from "svelte";
  import type { CloudStatusResponse } from "../../lib/types";
  import type { ConfigFields } from "../../lib/settings-toml";
  import { fetchCloudStatus, disconnectCloud, storeSecret } from "../../lib/api";
  import { toast } from "../../lib/toast.svelte";

  let {
    fields = $bindable(),
    simple = false,
  }: {
    fields: ConfigFields;
    simple?: boolean;
  } = $props();

  // ── Cloud ──────────────────────────────────────────────────────────

  let cloudStatus = $state<CloudStatusResponse | null>(null);
  let cloudLoading = $state(true);
  let cloudAction = $state(false);
  let manualTokenMode = $state(false);
  let manualToken = $state("");

  async function pollCloudStatus() {
    try {
      cloudStatus = await fetchCloudStatus();
    } catch {
      // fetch failures are non-critical
    } finally {
      cloudLoading = false;
    }
  }

  onMount(() => {
    void pollCloudStatus();
  });

  function handleCloudConnect() {
    const port = fields.gateway_port ?? "7700";
    window.open(`https://agent-residuum.com/connect?port=${port}`, "_blank");
  }

  async function handleCloudDisconnect() {
    cloudAction = true;
    try {
      await disconnectCloud();
      await pollCloudStatus();
    } catch {
      // disconnect failure visible via unchanged status
    } finally {
      cloudAction = false;
    }
  }

  function handleCloudReconnect() {
    fields.cloud_enabled = true;
  }

  async function handleSaveManualToken() {
    if (!manualToken.trim()) return;
    cloudAction = true;
    try {
      const result = await storeSecret("cloud_token", manualToken.trim());
      fields.cloud_token = result.reference;
      fields.cloud_enabled = true;
      manualToken = "";
      manualTokenMode = false;
    } catch {
      // store failure is visible via missing token
    } finally {
      cloudAction = false;
    }
  }

  function handleCloudRemoveAccount() {
    const previousToken = fields.cloud_token;
    const previousEnabled = fields.cloud_enabled;
    fields.cloud_token = "";
    fields.cloud_enabled = false;
    toast.success("Removed the Residuum Cloud account.", {
      label: "Undo",
      onClick: () => {
        fields.cloud_token = previousToken;
        fields.cloud_enabled = previousEnabled;
      },
    });
  }
</script>

<div class="settings-section">
  <!-- Residuum Cloud -->
  <div class="settings-group">
    <div class="settings-group-label">Residuum Cloud</div>
    <div class="integration-card">
      <div class="integration-desc">
        Connect your agent to <strong>Residuum Cloud</strong> for remote access via a personal subdomain.
        Your agent becomes accessible from anywhere without port forwarding or VPN setup.
      </div>

      {#if cloudLoading}
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-loading"></span>
          <span>Loading status...</span>
        </div>
      {:else if cloudStatus?.status === "connected"}
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-connected"></span>
          <span class="cloud-status-text">Connected</span>
          {#if cloudStatus.user_id}
            <span class="cloud-user-id">({cloudStatus.user_id})</span>
          {/if}
        </div>
        {#if cloudStatus.viewed_via_tunnel}
          <p class="cloud-hint">
            Disconnecting can't be done remotely, because nothing could bring Residuum back. Do it
            on the machine running Residuum.
          </p>
        {:else}
          <div class="cloud-actions">
            <button
              class="btn btn-sm btn-secondary"
              onclick={handleCloudDisconnect}
              disabled={cloudAction}
            >
              {cloudAction ? "Disconnecting..." : "Disconnect"}
            </button>
          </div>
        {/if}
      {:else if cloudStatus?.status === "connecting"}
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-connecting"></span>
          <span class="cloud-status-text">Connecting...</span>
        </div>
        {#if cloudStatus.viewed_via_tunnel}
          <p class="cloud-hint">
            Cancelling can't be done remotely, because nothing could bring Residuum back. Do it on
            the machine running Residuum.
          </p>
        {:else}
          <div class="cloud-actions">
            <button
              class="btn btn-sm btn-secondary"
              onclick={handleCloudDisconnect}
              disabled={cloudAction}
            >
              {cloudAction ? "Cancelling..." : "Cancel"}
            </button>
          </div>
        {/if}
      {:else if cloudStatus?.has_token && !cloudStatus?.enabled}
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-disconnected"></span>
          <span class="cloud-status-text">Disconnected</span>
        </div>
        <div class="cloud-actions">
          <button
            class="btn btn-sm btn-primary"
            onclick={handleCloudReconnect}
            disabled={cloudAction}
          >
            Reconnect
          </button>
          <button
            class="btn btn-sm btn-danger"
            onclick={handleCloudRemoveAccount}
            disabled={cloudAction}
          >
            Remove Account
          </button>
        </div>
        <p class="cloud-hint">Click Reconnect then Save to re-enable the tunnel.</p>
      {:else}
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-disconnected"></span>
          <span class="cloud-status-text">Not connected</span>
        </div>
        <div class="cloud-actions">
          <button class="btn btn-primary" onclick={handleCloudConnect}>
            Connect to Residuum Cloud
          </button>
        </div>

        {#if !manualTokenMode}
          <button
            class="cloud-manual-toggle"
            onclick={() => {
              manualTokenMode = true;
            }}
          >
            Use a token instead
          </button>
        {:else}
          <div class="cloud-manual-token">
            <label for="cloud-manual-token-input">Tunnel Token</label>
            <div class="cloud-manual-token-row">
              <input
                id="cloud-manual-token-input"
                type="password"
                bind:value={manualToken}
                placeholder="rst_..."
                onkeydown={(e) => {
                  if (e.key === "Enter") void handleSaveManualToken();
                }}
              />
              <button
                class="btn btn-sm btn-primary"
                onclick={handleSaveManualToken}
                disabled={cloudAction || !manualToken.trim()}
              >
                Save
              </button>
            </div>
          </div>
        {/if}
      {/if}
    </div>
  </div>

  <!-- Advanced cloud settings (only show if token exists) -->
  {#if !simple && (cloudStatus?.has_token === true || fields.cloud_token)}
    <div class="settings-group">
      <div class="settings-group-label">Cloud Advanced</div>
      <div class="integration-card">
        <div class="settings-field">
          <label for="cloud-relay-url">Relay URL</label>
          <input
            id="cloud-relay-url"
            type="text"
            bind:value={fields.cloud_relay_url}
            placeholder="wss://agent-residuum.com/tunnel/register (default)"
          />
        </div>
        <div class="settings-field">
          <label for="cloud-local-port">Local Port</label>
          <input
            id="cloud-local-port"
            type="text"
            bind:value={fields.cloud_local_port}
            placeholder="Same as gateway port (default)"
          />
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .cloud-status-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0;
  }

  .cloud-status-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .cloud-status-connected {
    background: #4ade80;
    box-shadow: 0 0 6px rgba(74, 222, 128, 0.4);
  }

  .cloud-status-connecting {
    background: #facc15;
    animation: pulse-dot 1.5s infinite;
  }

  .cloud-status-disconnected {
    background: #666;
  }

  .cloud-status-loading {
    background: #888;
    animation: pulse-dot 1.5s infinite;
  }

  @keyframes pulse-dot {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  .cloud-status-text {
    font-weight: 500;
  }

  .cloud-user-id {
    color: var(--text-dim, #888);
    font-size: 0.85rem;
  }

  .cloud-actions {
    display: flex;
    gap: 8px;
    margin: 8px 0;
  }

  .cloud-hint {
    font-size: 0.8rem;
    color: var(--text-dim, #888);
    margin-top: 4px;
  }

  .cloud-manual-toggle {
    background: none;
    border: none;
    color: var(--link, #7aa2f7);
    cursor: pointer;
    font-size: 0.85rem;
    padding: 4px 0;
    margin-top: 8px;
  }

  .cloud-manual-toggle:hover {
    text-decoration: underline;
  }

  .cloud-manual-token {
    margin-top: 12px;
  }

  .cloud-manual-token label {
    display: block;
    font-size: 0.85rem;
    color: var(--text-dim, #aaa);
    margin-bottom: 4px;
  }

  .cloud-manual-token-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .cloud-manual-token-row input {
    flex: 1;
  }
</style>
