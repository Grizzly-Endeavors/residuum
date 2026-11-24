<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { Banner, Button } from "../lib/ui";
</script>

{#if hub.transport.lost}
  <Banner tone="warn" edge title="Can't reach Residuum.">
    Trying again on its own. Agent states and counts may be out of date until it's back.
    {#snippet actions()}
      <Button
        size="sm"
        loading={hub.transport.status === "connecting"}
        onclick={() => hub.reconnectNow()}
      >
        Retry
      </Button>
    {/snippet}
  </Banner>
{/if}
