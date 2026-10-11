<script lang="ts">
  import type { SectionId } from "../../lib/settings-sections";
  import HistoryBrowser from "./HistoryBrowser.svelte";
  import type { SettingsScope } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // Settings → History for either scope: an agent's workspace and config
  // files, or for All agents the shared files and the install-wide config.

  let { scope, section }: { scope: SettingsScope; section: SectionId } = $props();
</script>

<SettingsSection
  {scope}
  {section}
  title="History"
  lede={scope.agent === null
    ? "Earlier versions of the shared files and the install-wide settings, kept as checkpoints. Restore a file, or undo everything a checkpoint changed."
    : `Earlier versions of ${scope.agent}'s files and settings, kept as checkpoints. Restore a file, or undo everything a checkpoint changed.`}
>
  <HistoryBrowser agent={scope.agent} />
</SettingsSection>
