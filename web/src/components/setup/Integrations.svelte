<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState, onNext, onBack }: Props = $props();
</script>

<h2>Integrations</h2>
<p class="subtitle">
  Optionally connect Discord and/or Telegram bots. You can skip this and add them later.
</p>

<div class="integration-card">
  <div class="integration-header">Discord</div>
  <div class="integration-desc">
    Connect a Discord bot so your agent can communicate via DMs. Create a bot at <a
      href="https://discord.com/developers/applications"
      target="_blank"
      rel="noopener">discord.com/developers</a
    >.
  </div>
  <div class="settings-field">
    <label for="setup-discord-token">Bot Token</label>
    <input
      id="setup-discord-token"
      type="password"
      bind:value={wizardState.integrations.discordToken}
      placeholder="Discord bot token (optional)"
    />
  </div>
</div>

<div class="integration-card">
  <div class="integration-header">Telegram</div>
  <div class="integration-desc">
    Connect a Telegram bot for DM-based interaction. Create a bot via <a
      href="https://t.me/BotFather"
      target="_blank"
      rel="noopener">@BotFather</a
    >.
  </div>
  <div class="settings-field">
    <label for="setup-telegram-token">Bot Token</label>
    <input
      id="setup-telegram-token"
      type="password"
      bind:value={wizardState.integrations.telegramToken}
      placeholder="Telegram bot token (optional)"
    />
  </div>
</div>

<p class="skip-hint">Both integrations are optional. You can add them later in settings.</p>

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack}>Back</button>
  <button class="btn btn-primary" onclick={onNext}>Next</button>
</div>
