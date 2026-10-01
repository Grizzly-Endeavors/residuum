<script lang="ts">
  import { router } from "../../lib/router.svelte";
  import { Button } from "../../lib/ui";
  import ConfigNumber from "./ConfigNumber.svelte";
  import ConfigToggle from "./ConfigToggle.svelte";
  import type { AgentSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The Schedule section: whether the agent's pulses run, and how
  // its background sessions behave. The Schedule place shows what is
  // scheduled; this is the `config.toml` that governs it, so it stays
  // editable while the agent is stopped.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);
</script>

{#snippet foot()}
  <Button
    variant="quiet"
    size="sm"
    icon="clock"
    onclick={() => void router.openPlace({ kind: "schedule", agent })}
  >
    Open {agent}'s Schedule
  </Button>
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Schedule"
  lede="Whether {agent} runs its pulses, and how long its background sessions stay open."
>
  <SettingsGroup
    title="Pulses"
    lede="Pulses are {agent}'s regular background checks, like looking through your inbox each morning."
    {foot}
  >
    <ConfigToggle
      {scope}
      field="pulse_enabled"
      label="Run pulses"
      hint="Turn this off to pause every pulse. Each pulse also has its own switch in Schedule."
    />
  </SettingsGroup>

  <SettingsGroup
    title="Background sessions"
    lede="Pulses, scheduled actions and helpers run as background sessions. Once a session has finished its work, it stays open this long for more messages, then closes."
  >
    <ConfigNumber
      {scope}
      field="bg_idle_timeout_scheduled_minutes"
      label="Pulses, scheduled actions and webhooks"
      unit="minutes"
      fallback={2}
      min={0}
      hint="Webhook calls are one-shot, so they close on this setting too."
    />
    <ConfigNumber
      {scope}
      field="bg_idle_timeout_spawned_minutes"
      label="Helper sessions"
      unit="minutes"
      fallback={10}
      min={0}
      hint="Sessions {agent} starts to hand a task off, including the ones that learn from conversations."
    />
    <ConfigNumber
      {scope}
      field="bg_idle_timeout_external_minutes"
      label="Chat conversations"
      unit="minutes"
      fallback={30}
      min={0}
      hint="Conversations in Discord, Telegram and Teams, and with other agents."
    />
    <ConfigNumber
      {scope}
      field="bg_idle_timeout_artifact_minutes"
      label="Workbench artifacts"
      unit="minutes"
      fallback={10}
      min={0}
      hint="Sessions an artifact in the Workbench starts."
    />
  </SettingsGroup>

  <SettingsGroup title="Keeping and nesting">
    <ConfigNumber
      {scope}
      field="bg_episode_skip_token_floor"
      label="Shortest session worth remembering"
      unit="tokens"
      fallback={2000}
      min={0}
      hint="A session shorter than this that kept nothing to remember doesn't become a memory. Its transcript is still saved."
    />
    <ConfigNumber
      {scope}
      field="bg_subagent_depth_cap"
      label="How deep helpers can nest"
      unit="levels"
      fallback={3}
      min={1}
      hint="Helpers can start helpers of their own. {agent} is level 0, and a session at the limit can't start another."
    />
  </SettingsGroup>
</SettingsSection>
