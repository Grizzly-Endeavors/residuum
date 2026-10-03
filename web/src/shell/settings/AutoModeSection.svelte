<script lang="ts">
  import { hub } from "../../lib/hub.svelte";
  import { router } from "../../lib/router.svelte";
  import { ALL_SCOPE } from "../../lib/settings-sections";
  import { Banner, Button, Disclosure } from "../../lib/ui";
  import ConfigNumber from "./ConfigNumber.svelte";
  import ConfigToggle from "./ConfigToggle.svelte";
  import RuleList from "./RuleList.svelte";
  import { configFieldError, type AgentSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The agent's Auto Mode: plain-language rules the decision model checks
  // each tool call against before it runs. A call matching a "not allowed"
  // rule, and no exception, is skipped, and the agent is told which rule.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);
  const enabled = $derived(scope.config.auto_mode_enabled);
  const status = $derived(hub.systemOne);

  let moreOpen = $state(false);
  const thresholdError = $derived(configFieldError(scope, "auto_mode_threshold"));
  $effect(() => {
    if (thresholdError !== undefined) moreOpen = true;
  });

  function openDecisionModel(): void {
    void router.openSettings({ scope: ALL_SCOPE, section: "system_one" });
  }
</script>

<SettingsSection
  {scope}
  {section}
  title="Auto Mode"
  lede="Rules in plain words for what {agent} isn't allowed to do. Before each tool call runs, a decision model checks it against them; a call that breaks one is skipped, and {agent} is told which rule it hit so it can try another way."
>
  {#if status !== null && !status.configured}
    <Banner>
      Auto Mode needs a decision model, and none is set up yet. Until one is, tool calls run without
      being checked.
      {#snippet actions()}
        <Button size="sm" onclick={openDecisionModel}>Set up a decision model</Button>
      {/snippet}
    </Banner>
  {:else if status?.outage}
    <Banner tone="warn">
      {status.outage.message} Until it answers, tool calls run without being checked.
      {#snippet actions()}
        <Button size="sm" onclick={openDecisionModel}>Decision model settings</Button>
      {/snippet}
    </Banner>
  {/if}

  <SettingsGroup>
    <ConfigToggle
      {scope}
      field="auto_mode_enabled"
      label="Check tool calls against these rules"
      hint="Applies to {agent}'s own turns and every session it runs. Each check is one quick request to the decision model."
    />
    {#if enabled && scope.config.auto_mode_deny.length === 0}
      <p class="auto-mode-note">Add a rule below; with none, nothing is checked.</p>
    {/if}
  </SettingsGroup>

  <SettingsGroup
    title="Not allowed"
    lede="Describe actions, not tools: “Pushing to the main branch”, “Sending email to anyone but me”, “Deleting files outside the workspace”."
  >
    <RuleList
      bind:rules={scope.config.auto_mode_deny}
      addLabel="Rule to add"
      placeholder="Pushing to the main branch"
      empty="No rules yet."
      error={configFieldError(scope, "auto_mode_deny")}
    />
  </SettingsGroup>

  <SettingsGroup
    title="Exceptions"
    lede="Narrower cases that are fine even though a rule above covers them: “Deleting files under tmp/”."
  >
    <RuleList
      bind:rules={scope.config.auto_mode_allow}
      addLabel="Exception to add"
      placeholder="Deleting files under tmp/"
      empty="No exceptions."
      error={configFieldError(scope, "auto_mode_allow")}
    />
  </SettingsGroup>

  <Disclosure summary="More options" bind:open={moreOpen}>
    <SettingsGroup>
      <ConfigNumber
        {scope}
        field="auto_mode_threshold"
        label="How sure the model must be"
        hint="The probability, from 0 to 1, at which a rule counts as matching. Lower catches more and blocks more calls that were fine; higher blocks less."
        fallback={0.5}
        min={0.05}
        max={1}
        step={0.05}
      />
    </SettingsGroup>
  </Disclosure>
</SettingsSection>

<style>
  .auto-mode-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }
</style>
