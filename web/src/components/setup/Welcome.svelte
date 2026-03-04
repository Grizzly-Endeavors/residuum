<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
  }

  let { wizardState, onNext }: Props = $props();
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
  <button class="btn btn-primary" onclick={onNext}>Next</button>
</div>
