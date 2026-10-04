<script lang="ts">
  import { Icon } from "../../../lib/icons";
  import { Banner, Button, Disclosure, Spinner, TextField, VisuallyHidden } from "../../../lib/ui";
  import { TEAMS_SETUP_PHASES, type TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    state: TeamsSetupState;
  }

  let { state }: Props = $props();
</script>

<div class="step-content">
  <!-- Screen reader live region for progress announcements and sign-in alerts -->
  <div class="sr-only" aria-live="polite" aria-atomic="true">
    {state.liveAnnouncement}
  </div>

  {#if state.lostContact}
    <Banner tone="warn" title="Lost contact with Residuum, retrying…">
      Last update was {state.timeSinceLastUpdate}. Setup is continuing on the server.
    </Banner>
  {/if}

  <div class="progress-header">
    <span class="progress-title">Provisioning Teams integration</span>
    <span class="progress-elapsed">Elapsed: {state.formatElapsed(state.job?.started_at)}</span>
  </div>

  <div class="phase-list" role="list">
    {#each TEAMS_SETUP_PHASES as item (item.phase)}
      {@const isCompleted = state.job?.completed_phases.includes(item.phase) ?? false}
      {@const isCurrent = state.job?.phase === item.phase}
      <div class="phase-row" data-current={isCurrent || undefined} role="listitem">
        <span class="phase-icon">
          {#if isCompleted}
            <Icon name="check" size={16} />
          {:else if isCurrent && state.job?.state === "running"}
            <Spinner size={14} />
          {:else if isCurrent && state.job?.state === "waiting_for_user"}
            <Icon name="info" size={16} />
          {:else if isCurrent && state.job?.state === "failed"}
            <Icon name="warning" size={16} />
          {:else}
            <span class="phase-bullet"></span>
          {/if}
        </span>
        <span class="phase-label">{item.label}</span>
        <span class="phase-status">
          {#if isCompleted}
            Completed
          {:else if isCurrent && state.job?.state === "running"}
            In progress...
          {:else if isCurrent && state.job?.state === "waiting_for_user"}
            Waiting for sign-in
          {:else if isCurrent && state.job?.state === "failed"}
            Failed
          {:else}
            Pending
          {/if}
        </span>
      </div>
    {/each}
  </div>

  {#if state.job?.state === "waiting_for_user" && state.job.sign_in !== null}
    <div class="sign-in-box">
      <Banner tone="info" title="Authentication required">
        Sign in to your Microsoft 365 tenant to authorize the Agents Toolkit.
      </Banner>

      <div class="sign-in-body">
        <a
          href={state.job.sign_in.login_url}
          target="_blank"
          rel="noopener noreferrer"
          class="sign-in-link-btn"
        >
          Sign in with Microsoft 365
          <Icon name="external-link" size={14} />
          <VisuallyHidden>(opens in a new tab)</VisuallyHidden>
        </a>

        <p class="sign-in-note">
          If your browser is on a different computer than Residuum, after signing in the page will
          fail to load — copy the address from the address bar and paste it below:
        </p>

        <div class="redirect-submit-row">
          <div class="redirect-field">
            <TextField
              label="Redirect URL"
              bind:value={state.redirectUrl}
              placeholder="http://localhost:... or paste redirect URL"
              error={state.redirectError ?? undefined}
            />
          </div>
          <Button
            variant="primary"
            loading={state.redirectSubmitting}
            disabled={state.redirectUrl.trim() === ""}
            onclick={() => void state.handleRedirectSubmit()}
          >
            Submit
          </Button>
        </div>
      </div>
    </div>
  {/if}

  {#if state.logs.length > 0}
    <div class="logs-wrapper">
      <Disclosure summary="Show logs ({state.logs.length} lines)" bind:open={state.logsOpen}>
        <div class="log-console">
          {#if state.trimmedLogsCount > 0}
            <div class="log-trimmed-note">
              {state.trimmedLogsCount} older log lines were trimmed.
            </div>
          {/if}
          {#each state.logs as line (line.seq)}
            <div class="log-row" data-stream={line.stream}>
              <span class="log-time">{line.at}</span>
              <span class="log-text">{line.text}</span>
            </div>
          {/each}
        </div>
      </Disclosure>
    </div>
  {/if}
</div>

<style>
  .step-content {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .progress-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }

  .progress-title {
    color: var(--color-text);
  }

  .progress-elapsed {
    color: var(--color-text-3);
    font-variant-numeric: tabular-nums;
  }

  .phase-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    padding: var(--space-12);
    background: var(--color-stone-2);
    border-radius: var(--corner-md);
  }

  .phase-row {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    padding: var(--space-6) var(--space-8);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .phase-row[data-current] {
    color: var(--color-text);
    background: var(--color-stone-3);
  }

  .phase-icon {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    color: var(--color-vein-bright);
  }

  .phase-bullet {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-text-3);
  }

  .phase-label {
    flex: 1;
    min-width: 0;
  }

  .phase-status {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
  }

  .sign-in-box {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-16);
    background: var(--color-stone-2);
    border: 1px solid var(--color-vein-line);
    border-radius: var(--corner-md);
  }

  .sign-in-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .sign-in-link-btn {
    display: inline-flex;
    align-items: center;
    align-self: flex-start;
    gap: var(--space-8);
    height: 32px;
    padding: 0 var(--space-12);
    background: var(--color-vein-dim);
    color: var(--color-on-accent);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    text-decoration: none;

    &:hover {
      background: var(--color-vein-hover);
    }
  }

  .sign-in-note {
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
  }

  .redirect-submit-row {
    display: flex;
    align-items: flex-end;
    gap: var(--space-8);
  }

  .redirect-field {
    flex: 1;
    min-width: 0;
  }

  .logs-wrapper {
    margin-top: var(--space-4);
  }

  .log-console {
    max-height: 200px;
    overflow-y: auto;
    padding: var(--space-8);
    background: var(--color-stone-1);
    border-radius: var(--corner-sm);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-tight);
  }

  .log-trimmed-note {
    padding: var(--space-4) 0;
    margin-bottom: var(--space-6);
    border-bottom: 1px dashed var(--color-line);
    color: var(--color-text-3);
    font-style: italic;
  }

  .log-row {
    display: flex;
    gap: var(--space-8);
    color: var(--color-text-2);
    white-space: pre-wrap;
    word-break: break-all;
  }

  .log-row[data-stream="stderr"] {
    color: var(--color-err-text);
  }

  .log-time {
    color: var(--color-text-3);
    flex-shrink: 0;
  }

  .log-text {
    flex: 1;
  }
</style>
