<script lang="ts">
  import { onMount } from "svelte";
  import { fetchTeamsSetupJob } from "../../lib/api";
  import { CloudConnection, teamsMessagingEndpoint } from "../../lib/cloud.svelte";
  import { userErrorMessage } from "../../lib/errors";
  import { hub } from "../../lib/hub.svelte";
  import { toast } from "../../lib/toast.svelte";
  import type { TeamsSetupJob, TeamsSetupState } from "../../lib/types";
  import { Badge, Banner, Button, IconButton, TextField } from "../../lib/ui";
  import ChannelGroup from "./ChannelGroup.svelte";
  import type { ChannelState } from "./channel-state";
  import ConfigNumber from "./ConfigNumber.svelte";
  import ConfigToggle from "./ConfigToggle.svelte";
  import RunningOnly from "./RunningOnly.svelte";
  import SecretConfigField from "./SecretConfigField.svelte";
  import { configFieldError, type AgentSectionProps } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";
  import TeamsSetupModal from "./teams/TeamsSetupModal.svelte";
  import WebhooksGroup from "./WebhooksGroup.svelte";

  // The agent's Connections section: the chat platforms people reach it on,
  // and the webhooks other services post to. What a channel connects with is
  // a credential, stored as a secret when the scope saves.

  let { scope, section }: AgentSectionProps = $props();

  type Channel = "discord" | "telegram" | "teams";

  const agent = $derived(scope.agent);
  const running = $derived(hub.isRunning(agent));
  const saved = $derived(scope.configFile.baseline);

  /** The channel's state as the saved settings give it, with a staged disconnect noted. Nothing is known while the agent isn't running. */
  function stateOf(onDisk: boolean, inForm: boolean): ChannelState | null {
    if (!running) return null;
    if (!onDisk) return "not-connected";
    return inForm ? "connected" : "disconnecting";
  }

  const discordOn = $derived(saved.discord_token !== "");
  const telegramOn = $derived(saved.telegram_token !== "");
  const teamsOn = $derived(
    saved.teams_app_id !== "" && saved.teams_tenant_id !== "" && saved.teams_app_password !== "",
  );
  const teamsFilled = $derived(
    [
      scope.config.teams_app_id,
      scope.config.teams_tenant_id,
      scope.config.teams_app_password,
    ].filter((value) => value !== "").length,
  );

  function disconnectTeams(): void {
    scope.config.teams_app_id = "";
    scope.config.teams_tenant_id = "";
    scope.config.teams_app_password = "";
  }

  // The messaging endpoint exists only while Residuum Cloud is connected and
  // the relay has announced this hub's origin and instance.
  const cloud = new CloudConnection();
  let teamsJob = $state<TeamsSetupJob | null>(null);
  let teamsJobError = $state<string | null>(null);
  let teamsModalOpen = $state(false);

  async function loadTeamsJob(): Promise<void> {
    try {
      teamsJob = await fetchTeamsSetupJob(agent);
      teamsJobError = null;
    } catch (err: unknown) {
      teamsJob = null;
      teamsJobError = userErrorMessage(err, {
        action: "Couldn't check Teams setup status.",
      });
    }
  }

  onMount(() => {
    cloud.follow();
    void loadTeamsJob();
  });

  function teamsJobTone(state: TeamsSetupState): "accent" | "neutral" | "positive" | "danger" {
    switch (state) {
      case "running":
      case "waiting_for_user":
        return "accent";
      case "succeeded":
        return "positive";
      case "failed":
        return "danger";
      case "cancelled":
        return "neutral";
    }
  }

  function teamsJobLabel(state: TeamsSetupState): string {
    switch (state) {
      case "running":
        return "Setup running";
      case "waiting_for_user":
        return "Action needed";
      case "succeeded":
        return "Configured via Toolkit";
      case "failed":
        return "Setup failed";
      case "cancelled":
        return "Setup cancelled";
    }
  }
  const endpoint = $derived.by(() => {
    const status = cloud.status;
    if (!teamsOn || status?.status !== "connected") return null;
    if (status == null) return null;
    const origin = status.origin;
    const instance = status.instance;
    if (origin == null || origin === "" || instance == null || instance === "") return null;
    return teamsMessagingEndpoint(origin, instance, agent);
  });

  async function copyEndpoint(address: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(address);
      toast.success("Copied the messaging endpoint.");
    } catch {
      toast.error("Couldn't copy the messaging endpoint. Select it and copy it instead.");
    }
  }
</script>

{#snippet reach(channel: Channel, othersHint: string, contextHint: string)}
  <ConfigToggle
    {scope}
    field={`${channel}_respond_to_others`}
    label="Let others talk to this agent"
    hint={othersHint}
  />
  <ConfigNumber
    {scope}
    field={`${channel}_context_messages`}
    label="Earlier messages to read"
    hint={contextHint}
    placeholder="20"
    min={0}
  />
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Connections"
  lede="The places people can talk to {agent} besides this app, and the webhooks that send it messages."
>
  {#if !running}
    <div class="connections-notice">
      <RunningOnly {agent} subject="which of these are connected" />
    </div>
  {/if}

  <ChannelGroup
    title="Discord"
    lede="{agent} can chat in direct messages, and in server channels when someone mentions it."
    state={stateOf(discordOn, scope.config.discord_token !== "")}
    ondisconnect={discordOn && scope.config.discord_token !== ""
      ? () => {
          scope.config.discord_token = "";
        }
      : undefined}
    guide={{
      href: "https://discord.com/developers/applications",
      label: "How to create a Discord bot",
    }}
  >
    <SecretConfigField
      label="Bot token"
      bind:value={scope.config.discord_token}
      saved={saved.discord_token}
      placeholder="Paste the token from the Discord developer portal"
      error={configFieldError(scope, "discord_token")}
    />
    {@render reach(
      "discord",
      `Off: only you, the first person to message the bot. On: anyone who can message the bot can talk to ${agent}.`,
      `When someone mentions ${agent} in a server channel, it reads this many earlier messages first.`,
    )}
  </ChannelGroup>

  <ChannelGroup
    title="Telegram"
    lede="{agent} can chat privately, and in groups when someone mentions it."
    state={stateOf(telegramOn, scope.config.telegram_token !== "")}
    ondisconnect={telegramOn && scope.config.telegram_token !== ""
      ? () => {
          scope.config.telegram_token = "";
        }
      : undefined}
    guide={{ href: "https://t.me/BotFather", label: "Create a bot with @BotFather" }}
  >
    <SecretConfigField
      label="Bot token"
      bind:value={scope.config.telegram_token}
      saved={saved.telegram_token}
      placeholder="Paste the token from @BotFather"
      error={configFieldError(scope, "telegram_token")}
    />
    {@render reach(
      "telegram",
      `Off: only you, the first person to message the bot. On: anyone who can message the bot can talk to ${agent}.`,
      `In groups, ${agent} reads this many earlier messages when it's mentioned. Needs privacy mode turned off in @BotFather, or the bot made a group admin.`,
    )}
  </ChannelGroup>

  <ChannelGroup
    title="Microsoft Teams"
    lede="{agent} can chat in direct messages, group chats and channels. Residuum Cloud gives the bot its messaging endpoint. A tunnel to the listener port works too."
    state={stateOf(teamsOn, teamsFilled === 3)}
    ondisconnect={teamsOn && teamsFilled > 0 ? disconnectTeams : undefined}
    guide={{
      href: "https://dev.teams.microsoft.com/bots",
      label: "Register a bot in the Teams developer portal",
    }}
  >
    <div class="teams-setup-row">
      <Button variant="secondary" size="sm" onclick={() => (teamsModalOpen = true)}>
        {teamsJob !== null ? "View setup" : "Set up with Agents Toolkit"}
      </Button>
      {#if teamsJob !== null}
        <Badge dot tone={teamsJobTone(teamsJob.state)}>{teamsJobLabel(teamsJob.state)}</Badge>
      {/if}
    </div>
    {#if teamsJobError !== null}
      <Banner tone="warn">
        <div class="teams-error-banner">
          <span>{teamsJobError}</span>
          <Button variant="quiet" size="sm" onclick={() => void loadTeamsJob()}>Retry</Button>
        </div>
      </Banner>
    {/if}

    <TextField
      label="App ID"
      bind:value={scope.config.teams_app_id}
      placeholder="11111111-2222-3333-4444-555555555555"
      autocomplete="off"
      spellcheck={false}
      code
      error={configFieldError(scope, "teams_app_id")}
    />
    <TextField
      label="Tenant ID"
      bind:value={scope.config.teams_tenant_id}
      placeholder="Directory (tenant) ID"
      autocomplete="off"
      spellcheck={false}
      code
      error={configFieldError(scope, "teams_tenant_id")}
    />
    <SecretConfigField
      label="Client secret"
      bind:value={scope.config.teams_app_password}
      saved={saved.teams_app_password}
      placeholder="Paste the client secret"
      error={configFieldError(scope, "teams_app_password")}
    />
    {#if teamsFilled > 0 && teamsFilled < 3}
      <Banner tone="warn">
        Teams stays off until the app ID, tenant ID and client secret are all filled in. Fill in all
        three, or clear them to leave Teams unconfigured.
      </Banner>
    {/if}
    {#if teamsOn && cloud.loadError !== ""}
      <Banner tone="error">{cloud.loadError}</Banner>
    {:else if endpoint !== null}
      <div class="address">
        <span class="address-label">Messaging endpoint</span>
        <div class="address-row">
          <code class="address-text">{endpoint}</code>
          <IconButton
            icon="copy"
            size="sm"
            label="Copy messaging endpoint"
            onclick={() => void copyEndpoint(endpoint)}
          />
        </div>
        <p class="note">
          Paste this into the bot's endpoint address in the Teams developer portal.
        </p>
      </div>
    {:else if teamsOn && cloud.status?.status === "connected"}
      <Banner tone="warn">
        This Residuum Cloud connection doesn't publish a Teams address yet. Update the relay, or
        point your own tunnel at the listener port.
      </Banner>
    {:else if teamsOn && cloud.status !== null}
      <Banner>
        Connect Residuum Cloud to get a messaging endpoint, or point your own tunnel at the listener
        port. Teams messages through Residuum Cloud can't arrive until it is connected.
      </Banner>
    {/if}
    {@render reach(
      "teams",
      `Off: only you, the first person to message the bot. On: coworkers can mention or message ${agent} too.`,
      `When someone mentions ${agent} in a group chat, it reads this many earlier messages first.`,
    )}
    <ConfigNumber
      {scope}
      field="teams_port"
      label="Listener port"
      hint="Only for your own tunnel. Residuum Cloud doesn't use this port."
      placeholder="7701"
    />
  </ChannelGroup>

  <WebhooksGroup {scope} />

  <TeamsSetupModal
    {agent}
    bind:open={teamsModalOpen}
    onsuccess={loadTeamsJob}
    onclose={loadTeamsJob}
  />
</SettingsSection>

<style>
  .connections-notice {
    max-width: 640px;
    margin-bottom: var(--space-16);
  }

  .teams-setup-row {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .teams-error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-8);
    width: 100%;
  }

  .address {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
    max-width: 100%;
  }

  .address-label,
  .note {
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .address-row {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .address-text {
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
  }
</style>
