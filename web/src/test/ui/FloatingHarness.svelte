<script lang="ts">
  import Dialog from "../../lib/ui/Dialog.svelte";
  import IconButton from "../../lib/ui/IconButton.svelte";
  import Menu from "../../lib/ui/Menu.svelte";
  import MenuItem from "../../lib/ui/MenuItem.svelte";
  import MenuSeparator from "../../lib/ui/MenuSeparator.svelte";
  import Popover from "../../lib/ui/Popover.svelte";
  import TooltipHost, { tooltip } from "../../lib/ui/TooltipHost.svelte";
  import { provideTooltips } from "../../lib/ui/tooltip";

  // A menu, a popover and tooltipped icon buttons under the tooltip provider,
  // with a dialog that holds one more, as a page would hold them.
  let { onselect = () => {} }: { onselect?: (label: string) => void } = $props();

  let autostart = $state(false);
  let dialogOpen = $state(false);

  provideTooltips(tooltip);
</script>

<button type="button">Before</button>
<Menu label="Manage atlas">
  {#snippet trigger(props)}
    <IconButton icon="more" label="Manage" {...props} />
  {/snippet}
  {#snippet heading()}atlas, running{/snippet}
  <MenuItem label="Open chat" onselect={() => onselect("Open chat")} />
  <MenuItem label="Start" disabled onselect={() => onselect("Start")} />
  <MenuItem label="Stop" onselect={() => onselect("Stop")} />
  <MenuItem label="Restart" onselect={() => onselect("Restart")} />
  <MenuItem
    label="Start automatically"
    checked={autostart}
    onselect={() => (autostart = !autostart)}
  />
  <MenuSeparator />
  <MenuItem label="Settings" onselect={() => onselect("Settings")} />
</Menu>
<Popover label="Model for atlas">
  {#snippet trigger(props)}
    <button type="button" {...props}>Model</button>
  {/snippet}
  <button type="button">claude-9</button>
  <button type="button">claude-9-fast</button>
</Popover>
<IconButton icon="settings" label="Settings" />
<IconButton icon="copy" label="Copy" tooltip="Copy the address" />
<button type="button" onclick={() => (dialogOpen = true)}>Open dialog</button>
<button type="button">After</button>
<Dialog bind:open={dialogOpen} title="Inside">
  <IconButton icon="reload" label="Reload" />
</Dialog>
<TooltipHost />
