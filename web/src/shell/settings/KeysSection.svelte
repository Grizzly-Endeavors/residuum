<script lang="ts">
  import AgentKeysGroup from "./AgentKeysGroup.svelte";
  import type { AllSectionProps } from "./sections";
  import SecretsGroup from "./SecretsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  let { scope, section }: AllSectionProps = $props();
</script>

<SettingsSection
  {scope}
  {section}
  title="Saved keys"
  lede="Keys and passwords your agents use, kept encrypted. Adding or removing one takes effect at once, and values can't be viewed after saving."
>
  <AgentKeysGroup />
  <SecretsGroup />
</SettingsSection>
