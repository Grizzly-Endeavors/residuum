<script lang="ts">
  import { Icon } from "../../../lib/icons";
  import { Badge, Banner, Button, Spinner, VisuallyHidden } from "../../../lib/ui";
  import type { TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    state: TeamsSetupState;
  }

  let { state }: Props = $props();
</script>

{#if state.loadingPrereqs}
  <div class="loading-state">
    <Spinner size={24} />
    <p>Checking prerequisites...</p>
  </div>
{:else if state.prereqsError !== null}
  <Banner tone="error" title="Prerequisites check failed">
    {state.prereqsError}
  </Banner>
  <div class="action-buttons-single">
    <Button variant="secondary" onclick={() => void state.loadInitial()}>Retry</Button>
  </div>
{:else if state.prereqs !== null}
  <div class="step-content">
    <p class="step-lede">
      Residuum uses Microsoft 365 Agents Toolkit to configure, provision, and package your Teams
      bot.
    </p>

    <div class="card">
      <h3 class="card-title">Environment status</h3>

      <div class="status-grid">
        <div class="status-item">
          <span class="status-label">Node.js:</span>
          <span class="status-value">
            {#if state.prereqs.node.found && state.prereqs.node.version !== null}
              <Badge tone="positive" dot>{state.prereqs.node.version}</Badge>
            {:else}
              <Badge tone="danger" dot>Missing</Badge>
            {/if}
          </span>
        </div>

        <div class="status-item">
          <span class="status-label">npm:</span>
          <span class="status-value">
            {#if state.prereqs.npm.found && state.prereqs.npm.version !== null}
              <Badge tone="positive" dot>{state.prereqs.npm.version}</Badge>
            {:else}
              <Badge tone="neutral" dot>Missing</Badge>
            {/if}
          </span>
        </div>

        <div class="status-item">
          <span class="status-label">Agents Toolkit CLI:</span>
          <span class="status-value">
            {#if state.prereqs.atk.installed}
              <Badge tone="positive" dot>
                Installed ({state.prereqs.atk.version ?? state.prereqs.atk.pinned_version})
              </Badge>
            {:else}
              <Badge tone="neutral" dot>Not installed</Badge>
            {/if}
          </span>
        </div>
      </div>

      {#if !state.prereqs.atk.installed}
        <p class="install-dir-note">
          CLI pinned version <code>{state.prereqs.atk.pinned_version}</code> will be installed to:
          <code>{state.prereqs.atk.install_dir}</code>
        </p>
      {/if}
    </div>

    {#if state.nodeMissing}
      <Banner tone="error" title="Node.js 18+ required">
        Node.js was not found on your system. Microsoft 365 Agents Toolkit requires Node.js {state
          .prereqs.min_node_version}
        or newer. Please install Node.js and restart Residuum.
      </Banner>
    {/if}

    {#if state.prereqs.teams_already_configured}
      <div class="checkbox-container">
        <Banner tone="warn">
          Teams is already configured for this agent. Setting up a new bot replaces the current
          Teams connection (old registrations stay in your tenant).
        </Banner>
        <label class="checkbox-row">
          <input type="checkbox" class="checkbox-input" bind:checked={state.replaceExisting} />
          <span>Replace existing Teams configuration for {state.agent}</span>
        </label>
      </div>
    {/if}

    {#if !state.prereqs.atk.installed}
      <div class="checkbox-container">
        <label class="checkbox-row">
          <input type="checkbox" class="checkbox-input" bind:checked={state.consentInstallCli} />
          <span>
            I agree to install @microsoft/teams-app-cli ({state.prereqs.atk.pinned_version}) in
            Residuum's tools directory
          </span>
        </label>
      </div>
    {/if}

    <div class="manual-guide-link-row">
      <a
        href={state.prereqs.manual_guide_url}
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
{/if}

<style>
  .step-content {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .step-lede {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
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

  .status-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: var(--space-12);
  }

  .status-item {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .status-label {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
  }

  .install-dir-note {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
    word-break: break-all;
  }

  .checkbox-container {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .checkbox-row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
    font-size: var(--font-size-sm);
    color: var(--color-text);
    cursor: pointer;
    user-select: none;
  }

  .checkbox-input {
    margin-top: var(--space-2);
    cursor: pointer;
    accent-color: var(--color-vein);

    &:focus-visible {
      outline: 2px solid var(--color-focus);
      outline-offset: 2px;
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

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-12);
    padding: var(--space-32) 0;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .action-buttons-single {
    display: flex;
    justify-content: flex-end;
  }
</style>
