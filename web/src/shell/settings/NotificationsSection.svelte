<script lang="ts">
  import { EmptyState } from "../../lib/ui";
  import type { AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Notifications: the section's place in the install-wide settings, with
  // nothing to set yet.

  let { scope, section }: AllSectionProps = $props();
</script>

<SettingsSection
  {scope}
  {section}
  title="Notifications"
  lede="Alerts from your agents on this device."
>
  <SettingsGroup>
    <EmptyState>Push notifications aren't available in this version of Residuum.</EmptyState>
  </SettingsGroup>
</SettingsSection>
