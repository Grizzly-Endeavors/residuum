<script lang="ts">
  import type { ToolCallState } from "../lib/types";

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
    <span class="tool-status" class:ok={call.status === "done"} class:err={call.status === "error"}>
      {call.status === "running" ? "running..." : call.status}
    </span>
  </div>
  <div class="tool-body">
    {call.arguments}{#if call.result}
      {call.result}{/if}
  </div>
</div>
