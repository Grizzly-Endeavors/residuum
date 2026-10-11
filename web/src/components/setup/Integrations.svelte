<script lang="ts">
  import type { SetupWizardState } from "../../lib/types";
  import { Banner, SecretField, TextField } from "../../lib/ui";
  import SetupGroup from "./SetupGroup.svelte";
  import SetupNav from "./SetupNav.svelte";

  interface Props {
    wizardState: SetupWizardState;
    onNext: () => void;
    onBack: () => void;
  }

  let { wizardState = $bindable(), onNext, onBack }: Props = $props();

  const NO_SECRET = { kind: "none" } as const;

  let validationMsg = $state("");

  function handleNext() {
    const { teamsAppId, teamsTenantId, teamsAppPassword } = wizardState.integrations;
    const filled = [teamsAppId, teamsTenantId, teamsAppPassword].filter((v) => v.trim()).length;
    if (filled > 0 && filled < 3) {
      validationMsg =
        "Fill in all three Teams fields (app ID, tenant ID and client secret), or clear them to skip Teams.";
      return;
    }
    validationMsg = "";
    onNext();
  }
</script>

<SetupGroup title="Discord">
  {#snippet hint()}
    Your agent chats in direct messages, and in server channels when someone mentions it. Create a
    bot in the <a href="https://discord.com/developers/applications" target="_blank" rel="noopener"
      >Discord developer portal</a
    >.
  {/snippet}
  <SecretField
    label="Bot token"
    source={NO_SECRET}
    bind:value={wizardState.integrations.discordToken}
    placeholder="Paste the bot's token"
  />
</SetupGroup>

<SetupGroup title="Telegram">
  {#snippet hint()}
    Your agent chats privately, and in groups when someone mentions it. Create a bot with
    <a href="https://t.me/BotFather" target="_blank" rel="noopener">@BotFather</a>.
  {/snippet}
  <SecretField
    label="Bot token"
    source={NO_SECRET}
    bind:value={wizardState.integrations.telegramToken}
    placeholder="Paste the token from @BotFather"
  />
</SetupGroup>

<SetupGroup title="Microsoft Teams">
  {#snippet hint()}
    Your agent chats in direct messages, group chats and channels. Register a bot in the
    <a href="https://dev.teams.microsoft.com/bots" target="_blank" rel="noopener"
      >Teams Developer Portal</a
    >, then point its messaging endpoint at a tunnel to this machine's Teams port. The Teams setup
    guide in the docs walks through it.
  {/snippet}
  <TextField
    label="App ID"
    bind:value={wizardState.integrations.teamsAppId}
    placeholder="Bot or Entra app ID"
    autocomplete="off"
    spellcheck="false"
  />
  <TextField
    label="Tenant ID"
    bind:value={wizardState.integrations.teamsTenantId}
    placeholder="Directory (tenant) ID"
    autocomplete="off"
    spellcheck="false"
  />
  <SecretField
    label="Client secret"
    source={NO_SECRET}
    bind:value={wizardState.integrations.teamsAppPassword}
    placeholder="The bot's client secret"
  />
</SetupGroup>

{#if validationMsg}
  <Banner tone="error">{validationMsg}</Banner>
{/if}

<SetupNav {onBack} onNext={handleNext} />
