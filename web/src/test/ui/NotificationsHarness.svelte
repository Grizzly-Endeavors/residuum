<script lang="ts">
  import Dialog from "../../lib/ui/Dialog.svelte";
  import RecentNotifications from "../../lib/ui/RecentNotifications.svelte";
  import ToastRegion from "../../lib/ui/ToastRegion.svelte";

  let open = $state(false);
  let dialogOpen = $state(false);
</script>

<button type="button" onclick={() => (open = true)}>Recent notifications</button>
<button type="button" onclick={() => (dialogOpen = true)}>Open dialog</button>
<RecentNotifications bind:open />
<Dialog bind:open={dialogOpen} title="Settings">
  <button type="button">Save</button>
</Dialog>
<ToastRegion />
