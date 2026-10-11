<script lang="ts">
  import IconButton from "../../lib/ui/IconButton.svelte";
  import { provideTooltips, type TooltipProvider } from "../../lib/ui/tooltip";

  // An IconButton under a tooltip provider, as the overlay layer sets one.
  let { provider, tooltip }: { provider: TooltipProvider; tooltip?: string | false } = $props();

  provideTooltips((text) => provider(text));
</script>

<IconButton icon="settings" label="Settings" {tooltip} />
