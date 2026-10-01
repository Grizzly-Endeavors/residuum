<script lang="ts">
  import { numberOfText } from "../../lib/settings-bind";
  import { Banner } from "../../lib/ui";
  import ConfigNumber from "./ConfigNumber.svelte";
  import type { AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Session limits: how many background turns run at once across every
  // agent, and how long a chain of agent-to-agent messages can grow. Staged
  // with the rest of the install-wide settings.

  let { scope, section }: AllSectionProps = $props();

  const concurrent = $derived(numberOfText(scope.config.bg_max_concurrent));
  const soft = $derived(numberOfText(scope.config.bg_hop_soft_limit));
  const hard = $derived(numberOfText(scope.config.bg_hop_hard_limit));
  // What each limit is while its box is empty (`src/config/hub_types.rs`).
  const DEFAULTS = { concurrent: 3, soft: 8, hard: 32 };
  // Flags for values that make the setting do nothing or block work; neither stops a save.
  const noTurns = $derived((concurrent ?? DEFAULTS.concurrent) < 1);
  const noticeNeverShows = $derived((soft ?? DEFAULTS.soft) >= (hard ?? DEFAULTS.hard));
</script>

<SettingsSection
  {scope}
  {section}
  title="Session limits"
  lede="How much background work can run at once, and how long a chain of messages between agents can grow. A hop is one agent handing a message to another."
>
  <SettingsGroup title="Background work">
    <ConfigNumber
      {scope}
      field="bg_max_concurrent"
      label="Turns at once"
      unit="turns"
      min={0}
      placeholder={String(DEFAULTS.concurrent)}
      hint="How many background sessions, across every agent, can be working through a turn at the same moment. The rest wait their turn. Takes effect when Residuum restarts."
    />
    {#if noTurns}
      <Banner tone="warn">With none allowed, background sessions can never run a turn.</Banner>
    {/if}
  </SettingsGroup>

  <SettingsGroup title="Messages between agents">
    <ConfigNumber
      {scope}
      field="bg_hop_soft_limit"
      label="Ask for fewer replies after"
      unit="hops"
      min={0}
      placeholder={String(DEFAULTS.soft)}
      hint="From this many hops on, a delivered message carries a note asking the receiver to reply only if a reply is really needed."
    />
    <ConfigNumber
      {scope}
      field="bg_hop_hard_limit"
      label="Stop delivering after"
      unit="hops"
      min={0}
      placeholder={String(DEFAULTS.hard)}
      hint="From this many hops on, a message between agents is refused, which stops messages from looping forever."
    />
    {#if noticeNeverShows}
      <Banner tone="warn">
        Messages are refused at the second limit, so the first one's note never appears.
      </Banner>
    {/if}
  </SettingsGroup>
</SettingsSection>
