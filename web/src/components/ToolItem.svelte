<script lang="ts">
  import type { ToolCallState } from "../lib/types";
  import ToolArgs from "./ToolArgs.svelte";

  let { call }: { call: ToolCallState } = $props();
  let open = $state(false);
</script>

<div class="tool-item" class:open>
  <div
    class="tool-header"
    onclick={() => (open = !open)}
    role="button"
    tabindex="0"
    onkeydown={(e) => {
      if (e.key === "Enter" || e.key === " ") open = !open;
    }}
  >
    <span class="tool-chevron">&#9654;</span>
    <span class="tool-name">{call.name}</span>
    <ToolArgs name={call.name} args={call.arguments} />
    <span class="tool-status" class:ok={call.status === "done"} class:err={call.status === "error"}>
      {call.status === "running" ? "running..." : call.status}
    </span>
  </div>
  {#if call.result}
    <div class="tool-body">{call.result}</div>
  {/if}
</div>
