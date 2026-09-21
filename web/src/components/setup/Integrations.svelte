<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState, onNext, onBack }: Props = $props();

  let validationMsg = $state("");

  function handleNext() {
    const { teamsAppId, teamsTenantId, teamsAppPassword } = wizardState.integrations;
    const filled = [teamsAppId, teamsTenantId, teamsAppPassword].filter((v) => v.trim()).length;
    if (filled > 0 && filled < 3) {
      validationMsg =
        "Fill in all three Teams fields (App ID, Tenant ID, Client Secret), or clear them to skip Teams.";
      return;
    }
    validationMsg = "";
    onNext();
  }
</script>

<h2>Integrations</h2>
<p class="subtitle">
  Optionally connect Discord, Telegram, and/or Microsoft Teams bots. You can skip this and add them
  later.
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

<div class="integration-card">
  <div class="integration-header">Microsoft Teams</div>
  <div class="integration-desc">
    Connect a Microsoft Teams bot so your agent can chat in DMs, group chats, and channels. Register
    a bot in the <a href="https://dev.teams.microsoft.com/bots" target="_blank" rel="noopener"
      >Teams Developer Portal</a
    >, then point its messaging endpoint at a tunnel to this machine's Teams port. See the Teams
    setup guide in the docs.
  </div>
  <div class="settings-field">
    <label for="setup-teams-app-id">App ID</label>
    <input
      id="setup-teams-app-id"
      type="text"
      bind:value={wizardState.integrations.teamsAppId}
      placeholder="Bot / Entra app ID (optional)"
    />
  </div>
  <div class="settings-field">
    <label for="setup-teams-tenant-id">Tenant ID</label>
    <input
      id="setup-teams-tenant-id"
      type="text"
      bind:value={wizardState.integrations.teamsTenantId}
      placeholder="Directory (tenant) ID (optional)"
    />
  </div>
  <div class="settings-field">
    <label for="setup-teams-app-password">Client Secret</label>
    <input
      id="setup-teams-app-password"
      type="password"
      bind:value={wizardState.integrations.teamsAppPassword}
      placeholder="Client secret (optional)"
    />
  </div>
</div>

<p class="skip-hint">All integrations are optional. You can add them later in settings.</p>

{#if validationMsg}
  <div class="validation-msg error">{validationMsg}</div>
{/if}

<div class="setup-nav">
  <button class="btn btn-secondary" onclick={onBack}>Back</button>
  <button class="btn btn-primary" onclick={handleNext}>Next</button>
</div>
