<script lang="ts">
  import A2a from "../../components/settings/A2a.svelte";
  import AgentKeys from "../../components/settings/AgentKeys.svelte";
  import History from "../../components/settings/History.svelte";
  import Integrations from "../../components/settings/Integrations.svelte";
  import Memory from "../../components/settings/Memory.svelte";
  import Providers from "../../components/settings/Providers.svelte";
  import Pulses from "../../components/settings/Pulses.svelte";
  import Runtime from "../../components/settings/Runtime.svelte";
  import Secrets from "../../components/settings/Secrets.svelte";
  import { sectionsOf, type SectionId } from "../../lib/settings-sections";
  import type { SettingsScope } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // A section that isn't rebuilt yet shows its legacy panels, bound to the
  // scope's forms, so their edits are staged and saved like any other. These
  // panels can't show a problem on a field, so every problem the last save
  // found for the section shows at its top.

  let { scope, section }: { scope: SettingsScope; section: SectionId } = $props();

  const entry = $derived(sectionsOf(scope.kind).find((candidate) => candidate.id === section));
  const problems = $derived(
    scope.diagnostics
      .filter((placed) => placed.section === null || placed.section === section)
      .map((placed) => placed.diagnostic),
  );
  const agentScope = $derived(scope.kind === "agent" ? scope : null);
  const allScope = $derived(scope.kind === "all" ? scope : null);
</script>

<SettingsSection
  {scope}
  {section}
  title={entry?.label ?? section}
  lede={`${entry?.description ?? ""}.`}
  {problems}
>
  <div data-legacy-view>
    {#if allScope !== null}
      {#if section === "notifications"}
        <p class="settings-placeholder">
          Push notifications aren't available in this version of Residuum.
        </p>
      {:else if section === "listener"}
        <A2a bind:fields={allScope.configFile.form} />
      {:else if section === "keys"}
        <Secrets />
        <AgentKeys />
      {:else if section === "history"}
        <History scope="hub" agent={null} />
      {/if}
    {:else if agentScope !== null}
      {@const agent = agentScope.agent}
      {#if section === "model"}
        <Providers
          bind:providers={agentScope.providersFile.form.providers}
          bind:models={agentScope.providersFile.form.models}
          {agent}
        />
      {:else if section === "connections"}
        <Integrations bind:fields={agentScope.configFile.form} part="channels" {agent} />
        <Integrations bind:fields={agentScope.configFile.form} part="webhooks" {agent} />
      {:else if section === "tools"}
        <Integrations bind:fields={agentScope.configFile.form} part="tools" {agent} />
      {:else if section === "memory"}
        <Memory bind:fields={agentScope.configFile.form} />
      {:else if section === "schedule"}
        <Pulses bind:fields={agentScope.configFile.form} />
      {:else if section === "runtime"}
        <Runtime bind:fields={agentScope.configFile.form} />
      {:else if section === "history"}
        <History scope="agent" {agent} />
      {/if}
    {/if}
  </div>
</SettingsSection>
