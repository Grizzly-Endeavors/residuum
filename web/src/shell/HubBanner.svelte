<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { Banner, Button } from "../lib/ui";

  // Shown across the top of the main region while the hub socket is down,
  // which is the only connection state the shell shows. The socket retries on
  // its own; Retry skips the wait.
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
