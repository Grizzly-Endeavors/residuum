<script lang="ts">
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import { confirmations } from "./confirm.svelte";

  // Shows the questions `confirmations.ask` queues. The shell mounts one.
</script>

{#if confirmations.current}
  {@const request = confirmations.current}
  <ConfirmDialog
    open
    {...request}
    onconfirm={() => {
      confirmations.answer(true);
    }}
    oncancel={() => {
      confirmations.answer(false);
    }}
  />
{/if}
