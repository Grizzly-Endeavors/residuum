<script lang="ts">
  import { appUpdate } from "../lib/app-update.svelte";
  import { router } from "../lib/router.svelte";
  import { Banner, Button } from "../lib/ui";

  // Shown across the top of the main region once the app was rebuilt and the
  // new version is waiting. The page keeps running the files it loaded until
  // Reload, which first asks about unsaved work through the router's guard.
</script>

{#if appUpdate.ready && !appUpdate.dismissed}
  <Banner tone="info" edge title="Update ready." ondismiss={() => appUpdate.dismiss()}>
    Reload to use the latest version of Residuum.
    {#snippet actions()}
      <Button
        size="sm"
        icon="reload"
        loading={appUpdate.applying}
        onclick={() => void appUpdate.apply(() => router.guard.confirmReload())}
      >
        Reload
      </Button>
    {/snippet}
  </Banner>
{/if}
