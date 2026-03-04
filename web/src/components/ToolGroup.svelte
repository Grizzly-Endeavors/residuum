<script lang="ts">
  import type { ToolCallState } from "../lib/types";
  import ToolItem from "./ToolItem.svelte";

  let { calls, verbose }: { calls: ToolCallState[]; verbose: boolean } =
    $props();
</script>

{#if verbose}
  <div class="tool-group">
    {#each calls as call (call.id)}
      <ToolItem {call} />
    {/each}
  </div>
{/if}
