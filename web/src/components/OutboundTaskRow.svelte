<script lang="ts">
  import { ws } from "../lib/ws.svelte";
  import { outboundDuration, outboundStateText } from "../lib/session-format";
  import type { OutboundA2aTaskSummary } from "../lib/types";
  import CategoryBadge from "./CategoryBadge.svelte";
  import { Icon } from "../lib/icons";

  let { task }: { task: OutboundA2aTaskSummary } = $props();

  let stopping = $derived(ws.sessions.outboundStopping.has(task.task_id));
  let unreachableStop = $derived(ws.sessions.outboundUnreachable.get(task.task_id));
  let waiting = $derived(task.state === "input_required" || task.state === "auth_required");
  let label = $derived(`a2a:${task.agent}`);
</script>

<!-- A task sent to another agent has no transcript here, so the row isn't
     selectable; its controls are the stop button and, once a stop couldn't
     reach the agent, "Stop watching". -->
<li>
  <div
    class="session-row outbound-row"
    class:state-running={!waiting && !task.unreachable_since}
    class:state-idle={waiting && !task.unreachable_since}
    class:failed={Boolean(task.unreachable_since)}
  >
    <span class="session-row-seam" aria-hidden="true"></span>
    <span class="session-row-top">
      <CategoryBadge category="a2a" />
      <span class="session-row-source" title="Sent by {task.sender_address}, task {task.task_id}"
        >{label}</span
      >
      <span class="session-row-time" title="Sent this long ago">
        {outboundDuration(task, ws.sessions.now)}
      </span>
      <button
        type="button"
        class="session-row-stop"
        title={stopping ? "Stopping…" : "Stop this task"}
        aria-label={stopping
          ? `Stopping the task sent to ${label}`
          : `Stop the task sent to ${label}`}
        disabled={stopping}
        onclick={() => void ws.sessions.stopOutbound(task.task_id)}
      >
        <Icon name="stop" size={10} />
      </button>
    </span>
    <span class="session-row-purpose">{task.status_text || `Task sent to ${label}`}</span>
    <span class="session-row-state">
      <span class="session-row-state-label">{outboundStateText(task, ws.sessions.now)}</span>
    </span>
    {#if unreachableStop}
      <span class="session-row-error">
        <Icon name="warning" size={12} />
        <span class="session-row-error-text">{unreachableStop}</span>
      </span>
      <button
        type="button"
        class="sessions-text-btn outbound-stop-watching"
        disabled={stopping}
        onclick={() => void ws.sessions.stopWatchingOutbound(task.task_id)}
      >
        {stopping ? "Stopping…" : "Stop watching"}
      </button>
    {/if}
  </div>
</li>
