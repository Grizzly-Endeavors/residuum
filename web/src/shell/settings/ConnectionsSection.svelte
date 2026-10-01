<script lang="ts">
  import { hub } from "../../lib/hub.svelte";
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import type { ConfigFields } from "../../lib/settings-toml";
  import { Banner, NumberField, TextField, Toggle } from "../../lib/ui";
  import ChannelGroup from "./ChannelGroup.svelte";
  import type { ChannelState } from "./channel-state";
  import RunningOnly from "./RunningOnly.svelte";
  import SecretConfigField from "./SecretConfigField.svelte";
  import { fieldError, type AgentSectionProps } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";
  import WebhooksGroup from "./WebhooksGroup.svelte";

  // The agent's Connections section: the chat platforms people reach it on,
  // and the webhooks other services post to. What a channel connects with is
  // a credential, stored as a secret when the scope saves.

  let { scope, section }: AgentSectionProps = $props();

  type Channel = "discord" | "telegram" | "teams";

  const agent = $derived(scope.agent);
  const running = $derived(hub.agent(agent)?.state === "running" && !hub.isStopping(agent));
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

  const problem = (field: keyof ConfigFields): string | undefined =>
    fieldError(scope, { kind: "config", field });

  function disconnectTeams(): void {
    scope.config.teams_app_id = "";
    scope.config.teams_tenant_id = "";
    scope.config.teams_app_password = "";
  }
</script>

{#snippet reach(channel: Channel, othersHint: string, contextHint: string)}
  <Toggle
    label="Let others talk to this agent"
    hint={othersHint}
    bind:checked={scope.config[`${channel}_respond_to_others`]}
    error={problem(`${channel}_respond_to_others`)}
  />
  <NumberField
    label="Earlier messages to read"
    hint={contextHint}
    bind:value={
      () => numberOfText(scope.config[`${channel}_context_messages`]),
      (value) => (scope.config[`${channel}_context_messages`] = textOfNumber(value))
    }
    placeholder="20"
    min={0}
    error={problem(`${channel}_context_messages`)}
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
      error={problem("discord_token")}
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
      error={problem("telegram_token")}
    />
    {@render reach(
      "telegram",
      `Off: only you, the first person to message the bot. On: anyone who can message the bot can talk to ${agent}.`,
      `In groups, ${agent} reads this many earlier messages when it's mentioned. Needs privacy mode turned off in @BotFather, or the bot made a group admin.`,
    )}
  </ChannelGroup>

  <ChannelGroup
    title="Microsoft Teams"
    lede="{agent} can chat in direct messages, group chats and channels. Point the bot's messaging endpoint at a tunnel to this machine's Teams port."
    state={stateOf(teamsOn, teamsFilled === 3)}
    ondisconnect={teamsOn && teamsFilled > 0 ? disconnectTeams : undefined}
    guide={{
      href: "https://dev.teams.microsoft.com/bots",
      label: "Register a bot in the Teams developer portal",
    }}
  >
    <TextField
      label="App ID"
      bind:value={scope.config.teams_app_id}
      placeholder="11111111-2222-3333-4444-555555555555"
      autocomplete="off"
      spellcheck={false}
      code
      error={problem("teams_app_id")}
    />
    <TextField
      label="Tenant ID"
      bind:value={scope.config.teams_tenant_id}
      placeholder="Directory (tenant) ID"
      autocomplete="off"
      spellcheck={false}
      code
      error={problem("teams_tenant_id")}
    />
    <SecretConfigField
      label="Client secret"
      bind:value={scope.config.teams_app_password}
      saved={saved.teams_app_password}
      placeholder="Paste the client secret"
      error={problem("teams_app_password")}
    />
    {#if teamsFilled > 0 && teamsFilled < 3}
      <Banner tone="warn">
        Teams stays off until the app ID, tenant ID and client secret are all filled in. Fill in all
        three, or clear them to leave Teams unconfigured.
      </Banner>
    {/if}
    {@render reach(
      "teams",
      `Off: only you, the first person to message the bot. On: coworkers can mention or message ${agent} too.`,
      `When someone mentions ${agent} in a group chat, it reads this many earlier messages first.`,
    )}
    <NumberField
      label="Listener port"
      hint="Expose only this port through your tunnel."
      bind:value={
        () => numberOfText(scope.config.teams_port),
        (value) => (scope.config.teams_port = textOfNumber(value))
      }
      placeholder="7701"
      error={problem("teams_port")}
    />
  </ChannelGroup>

  <WebhooksGroup {scope} />
</SettingsSection>

<style>
  .connections-notice {
    max-width: 640px;
    margin-bottom: var(--space-16);
  }
</style>
