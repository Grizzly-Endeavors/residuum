<script lang="ts">
  import Dialog from "../../lib/ui/Dialog.svelte";
  import Drawer from "../../lib/ui/Drawer.svelte";
  import Sheet from "../../lib/ui/Sheet.svelte";

  // A trigger and a modal layer of each kind, with a second dialog to open
  // from inside the first, as a page would hold them.
  let { kind = "dialog", onclose }: { kind?: "dialog" | "sheet" | "drawer"; onclose?: () => void } =
    $props();

  let open = $state(false);
  let inner = $state(false);
</script>

<button type="button" onclick={() => (open = true)}>Open</button>
<p>The page behind</p>

{#if kind === "dialog"}
  <Dialog bind:open title="Outer" description="The first layer." {onclose}>
    <button type="button">First control</button>
    <button type="button" onclick={() => (inner = true)}>Open inner</button>
    {#snippet actions()}
      <button type="button">Last control</button>
    {/snippet}
  </Dialog>
  <Dialog bind:open={inner} title="Inner">
    <button type="button">Inner control</button>
  </Dialog>
{:else if kind === "sheet"}
  <Sheet bind:open title="Switch agent" {onclose}>
    <button type="button">scout</button>
  </Sheet>
{:else}
  <Drawer bind:open label="Agents and places" {onclose}>
    <button type="button">Home</button>
  </Drawer>
{/if}
