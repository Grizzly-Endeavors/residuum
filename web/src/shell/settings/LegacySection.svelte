<script lang="ts">
  import Providers from "../../components/settings/Providers.svelte";
  import { AGENT_SECTIONS } from "../../lib/settings-sections";
  import type { AgentSectionProps } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // An agent section that isn't rebuilt yet shows its legacy panels, bound to
  // the scope's forms, so their edits are staged and saved like any other.
  // These panels can't show a problem on a field, so every problem the last
  // save found for the section shows at its top.

  let { scope, section }: AgentSectionProps = $props();

  const entry = $derived(AGENT_SECTIONS.find((candidate) => candidate.id === section));
  const problems = $derived(
    scope.diagnostics
      .filter((placed) => placed.section === null || placed.section === section)
      .map((placed) => placed.diagnostic),
  );
</script>

<SettingsSection
  {scope}
  {section}
  title={entry?.label ?? section}
  lede={`${entry?.description ?? ""}.`}
  {problems}
>
  <div data-legacy-view>
    {#if section === "model"}
      <Providers
        bind:providers={scope.providersFile.form.providers}
        bind:models={scope.providersFile.form.models}
        agent={scope.agent}
      />
    {/if}
  </div>
</SettingsSection>
