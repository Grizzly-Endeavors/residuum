<script lang="ts">
  import { Badge, Banner, Button } from "../../../lib/ui";
  import type { TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    state: TeamsSetupState;
  }

  let { state }: Props = $props();
</script>

<div class="step-content">
  <Banner tone="info" title="Teams setup complete">
    Microsoft Teams integration has been provisioned and configured for {state.agent}.
  </Banner>

  <div class="card">
    <h3 class="card-title">Configured Bot Resources</h3>
    <div class="resource-item">
      <span class="resource-label">Bot ID:</span>
      <code>{state.job?.result?.bot_id ?? state.job?.created.bot_id}</code>
    </div>
    <div class="resource-item">
      <span class="resource-label">Tenant ID:</span>
      <code>{state.job?.result?.tenant_id ?? state.job?.created.tenant_id}</code>
    </div>
  </div>

  <div class="done-actions-row">
    {#if state.job?.app_installed}
      <Badge tone="positive" dot>App installed in Teams</Badge>
    {:else}
      <Button
        variant="primary"
        loading={state.installingApp}
        onclick={() => void state.handleInstallApp()}
      >
        Install in Teams
      </Button>
    {/if}

    <Button variant="secondary" onclick={() => void state.handleDownloadPackage()}>
      Download app package
    </Button>
  </div>

  {#if state.installError !== null}
    <Banner tone="error">{state.installError}</Banner>
  {/if}

  <div class="owner-reminder-box">
    <p class="reminder-text">
      <strong>Direct message reminder:</strong> Before coworkers can message the bot, you (the owner)
      must initiate the first direct message with the bot in Microsoft Teams, unless "Let others talk
      to this agent" is enabled.
    </p>
  </div>

  <div class="cleanup-box">
    <Button
      variant="quiet"
      size="sm"
      loading={state.cleanupLoading}
      disabled={state.cleanupSuccess}
      onclick={() => void state.handleCleanup()}
    >
      {state.cleanupSuccess ? "Temporary files cleaned up" : "Clean up temporary files"}
    </Button>
  </div>
</div>

<style>
  .step-content {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-16);
    background: var(--color-stone-2);
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
  }

  .card-title {
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text);
  }

  .resource-item {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    font-size: var(--font-size-sm);
  }

  .resource-label {
    color: var(--color-text-3);
  }

  .done-actions-row {
    display: flex;
    align-items: center;
    gap: var(--space-12);
  }

  .owner-reminder-box {
    padding: var(--space-12);
    background: var(--color-stone-2);
    border-left: 3px solid var(--color-vein-bright);
    border-radius: var(--corner-sm);
  }

  .reminder-text {
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
  }

  .cleanup-box {
    margin-top: var(--space-4);
  }
</style>
