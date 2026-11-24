<script lang="ts">
  import IconButton from "../../lib/ui/IconButton.svelte";
  import { provideTooltips, type TooltipProvider } from "../../lib/ui/tooltip";

  let { provider, tooltip }: { provider: TooltipProvider; tooltip?: string | false } = $props();

  provideTooltips((text) => provider(text));
</script>

<IconButton icon="settings" label="Settings" {tooltip} />
