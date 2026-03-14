<script lang="ts">
  import { onMount } from "svelte";
  import type { CloudStatusResponse } from "../../lib/types";
  import type { ConfigFields } from "../../lib/settings-toml";
  import { fetchCloudStatus, disconnectCloud, storeSecret } from "../../lib/api";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();

  let cloudStatus = $state<CloudStatusResponse | null>(null);
  let loading = $state(true);
  let actionInProgress = $state(false);
  let manualTokenMode = $state(false);
  let manualToken = $state("");

  const POLL_INTERVAL = 5000;
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function pollStatus() {
    try {
      cloudStatus = await fetchCloudStatus();
    } catch {
      // Polling failures are non-critical
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void pollStatus();
    pollTimer = setInterval(() => void pollStatus(), POLL_INTERVAL);
    return () => {
      if (pollTimer) clearInterval(pollTimer);
    };
  });

  function gatewayPort(): string {
    return fields.gateway_port ?? "7700";
  }

  function handleConnect() {
    const port = gatewayPort();
    window.open(`https://agent-residuum.com/connect?port=${port}`, "_blank");
  }

  async function handleDisconnect() {
    actionInProgress = true;
    try {
      await disconnectCloud();
      cloudStatus = {
        status: "disconnected" as const,
        user_id: cloudStatus?.user_id ?? null,
        has_token: cloudStatus?.has_token ?? false,
        enabled: false,
      };
    } catch {
      // disconnect failure handled by status poll
    } finally {
      actionInProgress = false;
    }
  }

  function handleReconnect() {
    fields.cloud_enabled = true;
    // The save will be handled by the parent Settings component
  }

  async function handleSaveManualToken() {
    if (!manualToken.trim()) return;
    actionInProgress = true;
    try {
      const result = await storeSecret("cloud_token", manualToken.trim());
      fields.cloud_token = result.reference;
      fields.cloud_enabled = true;
      manualToken = "";
      manualTokenMode = false;
    } catch {
      // store failure is visible via missing token
    } finally {
      actionInProgress = false;
    }
  }

  function handleRemoveAccount() {
    fields.cloud_token = "";
    fields.cloud_enabled = false;
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Residuum Cloud</div>
    <div class="integration-card">
      <div class="integration-desc">
        Connect your agent to <strong>Residuum Cloud</strong> for remote access via a personal subdomain.
        Your agent becomes accessible from anywhere without port forwarding or VPN setup.
      </div>

      {#if loading}
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-loading"></span>
          <span>Loading status...</span>
        </div>
      {:else if cloudStatus?.status === "connected"}
        <!-- Connected state -->
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-connected"></span>
          <span class="cloud-status-text">Connected</span>
          {#if cloudStatus.user_id}
            <span class="cloud-user-id">({cloudStatus.user_id})</span>
          {/if}
        </div>
        <div class="cloud-actions">
          <button
            class="btn btn-sm btn-secondary"
            onclick={handleDisconnect}
            disabled={actionInProgress}
          >
            {actionInProgress ? "Disconnecting..." : "Disconnect"}
          </button>
        </div>
      {:else if cloudStatus?.status === "connecting"}
        <!-- Connecting state -->
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-connecting"></span>
          <span class="cloud-status-text">Connecting...</span>
        </div>
        <div class="cloud-actions">
          <button
            class="btn btn-sm btn-secondary"
            onclick={handleDisconnect}
            disabled={actionInProgress}
          >
            {actionInProgress ? "Cancelling..." : "Cancel"}
          </button>
        </div>
      {:else if cloudStatus?.has_token && !cloudStatus?.enabled}
        <!-- Disconnected with token, disabled -->
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-disconnected"></span>
          <span class="cloud-status-text">Disconnected</span>
        </div>
        <div class="cloud-actions">
          <button
            class="btn btn-sm btn-primary"
            onclick={handleReconnect}
            disabled={actionInProgress}
          >
            Reconnect
          </button>
          <button
            class="btn btn-sm btn-danger"
            onclick={handleRemoveAccount}
            disabled={actionInProgress}
          >
            Remove Account
          </button>
        </div>
        <p class="cloud-hint">Click Reconnect then Save to re-enable the tunnel.</p>
      {:else}
        <!-- Disconnected, no token -->
        <div class="cloud-status-row">
          <span class="cloud-status-dot cloud-status-disconnected"></span>
          <span class="cloud-status-text">Not connected</span>
        </div>
        <div class="cloud-actions">
          <button class="btn btn-primary" onclick={handleConnect}>
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
                disabled={actionInProgress || !manualToken.trim()}
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
  {#if cloudStatus?.has_token === true || fields.cloud_token}
    <div class="settings-group">
      <div class="settings-group-label">Advanced</div>
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
