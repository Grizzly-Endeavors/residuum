<script lang="ts">
  import { onMount } from "svelte";
  import type { UpdateStatusResponse } from "../../lib/types";
  import { fetchUpdateStatus, triggerUpdateCheck, applyUpdate } from "../../lib/api";

  let status = $state<UpdateStatusResponse | null>(null);
  let loading = $state(true);
  let checking = $state(false);
  let applying = $state(false);
  let restarting = $state(false);
  let errorMsg = $state("");

  const POLL_INTERVAL = 30_000;
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function pollStatus() {
    try {
      status = await fetchUpdateStatus();
      errorMsg = "";
    } catch {
      // polling failures are non-critical
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

  async function handleCheck() {
    checking = true;
    errorMsg = "";
    try {
      status = await triggerUpdateCheck();
    } catch (e: unknown) {
      errorMsg = `Check failed: ${String(e)}`;
    } finally {
      checking = false;
    }
  }

  async function handleApply() {
    applying = true;
    errorMsg = "";
    try {
      await applyUpdate();
      applying = false;
      restarting = true;
    } catch (e: unknown) {
      errorMsg = `Update failed: ${String(e)}`;
      applying = false;
    }
  }

  function relativeTime(iso: string): string {
    const now = Date.now();
    const then = new Date(iso).getTime();
    const diff = Math.floor((now - then) / 1000);
    if (diff < 60) return "just now";
    if (diff < 3600) {
      const m = Math.floor(diff / 60);
      return `${m}m ago`;
    }
    if (diff < 86400) {
      const h = Math.floor(diff / 3600);
      return `${h}h ago`;
    }
    const d = Math.floor(diff / 86400);
    return `${d}d ago`;
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Version</div>
    <div class="integration-card">
      {#if loading}
        <div class="update-status-row">
          <span class="update-dot update-dot-loading"></span>
          <span class="update-status-text">Loading...</span>
        </div>
      {:else if restarting}
        <div class="update-status-row">
          <span class="update-dot update-dot-restarting"></span>
          <span class="update-status-text">Restarting...</span>
        </div>
        <p class="update-hint">
          The gateway is restarting with the new version. This page will reconnect automatically.
        </p>
      {:else if status}
        <div class="update-status-row">
          {#if status.update_available}
            <span class="update-dot update-dot-available"></span>
            <span class="update-status-text">Update available</span>
          {:else if status.latest}
            <span class="update-dot update-dot-current"></span>
            <span class="update-status-text">Up to date</span>
          {:else}
            <span class="update-dot update-dot-unknown"></span>
            <span class="update-status-text">Unknown</span>
          {/if}
        </div>

        <div class="update-version-table">
          <div class="update-version-row">
            <span class="update-version-label">Current</span>
            <span class="update-version-value">{status.current}</span>
          </div>
          {#if status.latest}
            <div class="update-version-row">
              <span class="update-version-label">Latest</span>
              <span class="update-version-value" class:update-version-new={status.update_available}
                >{status.latest}</span
              >
            </div>
          {/if}
          {#if status.last_checked}
            <div class="update-version-row">
              <span class="update-version-label">Checked</span>
              <span class="update-version-value update-version-dim"
                >{relativeTime(status.last_checked)}</span
              >
            </div>
          {/if}
        </div>

        <div class="update-actions">
          <button
            class="btn btn-sm btn-secondary"
            onclick={handleCheck}
            disabled={checking || applying}
          >
            {checking ? "Checking..." : "Check Now"}
          </button>
          {#if status.update_available}
            <button
              class="btn btn-sm btn-primary"
              onclick={handleApply}
              disabled={applying || checking}
            >
              {applying ? "Updating..." : "Update & Restart"}
            </button>
          {/if}
        </div>
      {/if}

      {#if errorMsg}
        <p class="update-error">{errorMsg}</p>
      {/if}
    </div>
  </div>
</div>

<style>
  .update-status-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 14px;
  }

  .update-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .update-dot-current {
    background: #4ade80;
    box-shadow: 0 0 6px rgba(74, 222, 128, 0.4);
  }

  .update-dot-available {
    background: #facc15;
    box-shadow: 0 0 6px rgba(250, 204, 21, 0.3);
  }

  .update-dot-unknown {
    background: #666;
  }

  .update-dot-loading,
  .update-dot-restarting {
    background: #888;
    animation: pulse-update-dot 1.5s infinite;
  }

  @keyframes pulse-update-dot {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }

  .update-status-text {
    font-weight: 500;
    font-size: 13px;
  }

  .update-version-table {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 14px;
  }

  .update-version-row {
    display: flex;
    align-items: baseline;
    gap: 12px;
  }

  .update-version-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim, #6a6a6f);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    width: 64px;
    flex-shrink: 0;
  }

  .update-version-value {
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    color: var(--text, #e8e8ea);
  }

  .update-version-new {
    color: #facc15;
  }

  .update-version-dim {
    color: var(--text-muted, #9a9a9f);
    font-family: var(--font-body, serif);
  }

  .update-actions {
    display: flex;
    gap: 8px;
  }

  .update-hint {
    font-size: 12px;
    color: var(--text-dim, #6a6a6f);
    margin-top: 8px;
    line-height: 1.4;
  }

  .update-error {
    font-size: 12px;
    color: var(--error, #c0392b);
    margin-top: 10px;
  }
</style>
