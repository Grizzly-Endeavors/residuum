<script lang="ts">
  import { Icon } from "../../../lib/icons";
  import { Banner, Disclosure, VisuallyHidden } from "../../../lib/ui";
  import type { TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    state: TeamsSetupState;
  }

  let { state }: Props = $props();
</script>

<div class="step-content">
  <Banner
    tone="error"
    title={state.job?.state === "cancelled" ? "Setup cancelled" : "Setup failed"}
  >
    {#if state.job?.state === "cancelled"}
      The Teams setup operation was cancelled.
    {:else}
      {state.job?.error?.message ?? "An unexpected error occurred during Teams setup."}
    {/if}
  </Banner>

  {#if state.job?.error?.phase !== undefined}
    <p class="failed-phase-note">
      Failed during phase: <strong>{state.job.error.phase}</strong>
    </p>
  {/if}

  {#if state.job?.error?.detail !== null && state.job?.error?.detail !== undefined}
    <Disclosure summary="Technical error details" bind:open={state.errorDetailsOpen}>
      <pre class="error-detail-content">{state.job.error.detail}</pre>
    </Disclosure>
  {/if}

  {#if state.job?.created !== undefined && (state.job.created.bot_id !== null || state.job.created.teams_app_id !== null || state.job.created.tenant_id !== null)}
    <div class="card">
      <h3 class="card-title">Created tenant resources</h3>

      {#if state.job.created.bot_id !== null}
        <div class="resource-item">
          <span class="resource-label">Bot ID:</span>
          <code>{state.job.created.bot_id}</code>
          <a
            href={state.job.created.entra_url}
            target="_blank"
            rel="noopener noreferrer"
            class="resource-link"
          >
            Entra Admin Center
            <Icon name="external-link" size={13} />
            <VisuallyHidden>(opens in a new tab)</VisuallyHidden>
          </a>
        </div>
      {/if}

      {#if state.job.created.teams_app_id !== null}
        <div class="resource-item">
          <span class="resource-label">Teams App ID:</span>
          <code>{state.job.created.teams_app_id}</code>
          <a
            href={state.job.created.dev_portal_url}
            target="_blank"
            rel="noopener noreferrer"
            class="resource-link"
          >
            Developer Portal
            <Icon name="external-link" size={13} />
            <VisuallyHidden>(opens in a new tab)</VisuallyHidden>
          </a>
        </div>
      {/if}

      {#if state.job.created.tenant_id !== null}
        <div class="resource-item">
          <span class="resource-label">Tenant ID:</span>
          <code>{state.job.created.tenant_id}</code>
        </div>
      {/if}
    </div>
  {/if}

  <div class="manual-guide-link-row">
    <a
      href={state.prereqs?.manual_guide_url ?? "https://dev.teams.microsoft.com/bots"}
      target="_blank"
      rel="noopener noreferrer"
      class="guide-link"
    >
      Manual setup guide
      <Icon name="external-link" size={13} />
      <VisuallyHidden>(opens in a new tab)</VisuallyHidden>
    </a>
  </div>
</div>

<style>
  .step-content {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .failed-phase-note {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .error-detail-content {
    padding: var(--space-8);
    background: var(--color-stone-1);
    border-radius: var(--corner-sm);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    color: var(--color-err-text);
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 200px;
    overflow-y: auto;
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

  .resource-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    font-size: var(--font-size-xs);
    color: var(--color-vein-bright);
    text-decoration: none;

    &:hover {
      text-decoration: underline;
    }
  }

  .manual-guide-link-row {
    display: flex;
    align-items: center;
  }

  .guide-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    font-size: var(--font-size-sm);
    color: var(--color-vein-bright);
    text-decoration: none;

    &:hover {
      text-decoration: underline;
    }
  }
</style>
