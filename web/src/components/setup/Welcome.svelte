<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";
  import { agentNameProblem } from "../../lib/agent-name";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
  }

  let { wizardState, onNext }: Props = $props();

  let agentNameError = $derived(agentNameProblem(wizardState.agentName));
</script>

<h2>Welcome to Residuum</h2>
<p class="subtitle">Let's get your agent configured. This will only take a minute.</p>

<div class="settings-field">
  <label for="welcome-name">Your Name</label>
  <input
    id="welcome-name"
    type="text"
    bind:value={wizardState.userName}
    placeholder="What should your agent call you?"
  />
</div>

<div class="settings-field">
  <label for="welcome-agent-name">Agent Name</label>
  <input
    id="welcome-agent-name"
    type="text"
    bind:value={wizardState.agentName}
    placeholder="assistant"
    autocapitalize="off"
    spellcheck="false"
    aria-invalid={agentNameError !== null}
    aria-describedby="welcome-agent-name-hint"
  />
  {#if agentNameError}
    <div id="welcome-agent-name-hint" class="validation-msg error" role="alert">
      {agentNameError}
    </div>
  {:else}
    <span id="welcome-agent-name-hint" class="field-hint">
      Lowercase letters, digits, and hyphens. This is your agent's permanent name and folder.
    </span>
  {/if}
</div>

<div class="settings-field">
  <label for="welcome-timezone">Timezone (IANA format)</label>
  <input
    id="welcome-timezone"
    type="text"
    bind:value={wizardState.timezone}
    placeholder="America/New_York"
  />
</div>

<div class="setup-nav">
  <div></div>
  <button class="btn btn-primary" onclick={onNext} disabled={agentNameError !== null}>Next</button>
</div>
