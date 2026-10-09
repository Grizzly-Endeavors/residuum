<script lang="ts">
  import { SelectField, type Choice } from "../../lib/ui";
  import ConfigNumber from "./ConfigNumber.svelte";
  import ConfigToggle from "./ConfigToggle.svelte";
  import { fieldError, type AgentSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The Runtime section: limits on a reply, retries, what the
  // agent may change about itself, the caps on a turn, and what happens when
  // you go quiet. The default temperature and thinking level are in Model.
  // Everything is in the agent's `config.toml`, so it stays editable while the
  // agent is stopped.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);
  const guarding = $derived(scope.config.agent_repeat_call_guard_enabled);

  const CHANNELS: readonly Choice[] = [
    { value: "", label: "Keep where it is" },
    { value: "websocket", label: "This app" },
    { value: "telegram", label: "Telegram" },
    { value: "discord", label: "Discord" },
    { value: "teams", label: "Microsoft Teams" },
  ];

  // A channel the file names that the list doesn't (`ws` is another name for the app) still shows.
  const channels = $derived(
    CHANNELS.some((choice) => choice.value === scope.config.idle_channel)
      ? CHANNELS
      : [...CHANNELS, { value: scope.config.idle_channel, label: scope.config.idle_channel }],
  );
</script>

<SettingsSection
  {scope}
  {section}
  title="Runtime"
  lede="How long {agent} waits on its model, how much it can say in a reply, and what it may change about itself."
>
  <SettingsGroup title="Replies">
    <ConfigNumber
      {scope}
      field="timeout_secs"
      label="Reply time limit"
      unit="seconds"
      fallback={120}
      min={1}
      hint="How long {agent} waits for the model to answer before it gives up on that request."
    />
    <ConfigNumber
      {scope}
      field="max_tokens"
      label="Reply length"
      unit="tokens"
      fallback={16384}
      min={1}
      hint="The most the model can write in one go. A token is about three quarters of a word."
    />
  </SettingsGroup>

  <SettingsGroup
    title="Retries"
    lede="When a request to the model fails, {agent} tries again after a pause that grows each time."
  >
    <ConfigNumber
      {scope}
      field="retry_max_retries"
      label="Tries after a failure"
      unit="times"
      fallback={3}
      min={0}
      hint="How many times to try again before giving up."
    />
    <ConfigNumber
      {scope}
      field="retry_initial_delay_ms"
      label="First pause"
      unit="milliseconds"
      fallback={500}
      min={0}
      hint="How long to wait before the first new try."
    />
    <ConfigNumber
      {scope}
      field="retry_max_delay_ms"
      label="Longest pause"
      unit="milliseconds"
      fallback={30000}
      min={0}
      hint="Pauses stop growing at this length."
    />
    <ConfigNumber
      {scope}
      field="retry_backoff_multiplier"
      label="Each pause lasts"
      unit="× the one before"
      fallback={2}
      min={1}
      step={0.1}
      hint="How quickly the pauses grow."
    />
  </SettingsGroup>

  <SettingsGroup
    title="What it may change"
    lede="Lets {agent} edit these parts of its own setup when you ask it to."
  >
    <ConfigToggle
      {scope}
      field="agent_modify_mcp"
      label="Tool servers"
      hint="It can add and remove the tool servers it connects to."
    />
    <ConfigToggle
      {scope}
      field="agent_modify_channels"
      label="Notification channels"
      hint="It can add and remove the places its alerts are sent."
    />
  </SettingsGroup>

  <SettingsGroup
    title="Stopping runaway turns"
    lede="You can stop a turn yourself at any time. These stop it automatically."
  >
    <ConfigNumber
      {scope}
      field="agent_max_tool_iterations"
      label="Tool calls per turn"
      unit="tool calls"
      placeholder="No limit"
      min={1}
      hint="End a turn after this many tool calls. Leave blank for no limit."
    />
    <ConfigToggle
      {scope}
      field="agent_repeat_call_guard_enabled"
      label="Catch repeated tool calls"
      hint="Watches for a model making the exact same tool call, with the same details, over and over."
    />
    <ConfigNumber
      {scope}
      field="agent_repeat_call_steer_after"
      label="Nudge after"
      unit="identical calls in a row"
      fallback={3}
      min={1}
      disabled={!guarding}
      hint="The call still runs, with a note telling the model to try something else."
    />
    <ConfigNumber
      {scope}
      field="agent_repeat_call_stop_after"
      label="Stop after"
      unit="identical calls in a row"
      fallback={6}
      min={1}
      disabled={!guarding}
      hint="The turn ends instead of running the call again."
    />
  </SettingsGroup>

  <SettingsGroup
    title="When you're away"
    lede="After a quiet spell, {agent} saves the conversation to memory and starts the next one fresh."
  >
    <ConfigNumber
      {scope}
      field="idle_timeout_minutes"
      label="Quiet for"
      unit="minutes"
      fallback={30}
      min={0}
      hint="How long without a message from you counts as away. 0 turns this off."
    />
    <SelectField
      label="Send its updates to"
      options={channels}
      error={fieldError(scope, { kind: "config", field: "idle_channel" })}
      hint="Where messages {agent} starts on its own go once you're away."
      bind:value={
        () => scope.config.idle_channel,
        (next) => {
          scope.config.idle_channel = next;
        }
      }
    />
  </SettingsGroup>
</SettingsSection>
