<script lang="ts">
  import AgentKeysGroup from "./AgentKeysGroup.svelte";
  import type { AllSectionProps } from "./sections";
  import SecretsGroup from "./SecretsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Saved keys: two lists the hub keeps encrypted and every agent on this
  // install shares. Both act at once through their own endpoints, so nothing
  // here is staged for the save bar.

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
