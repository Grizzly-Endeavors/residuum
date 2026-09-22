<script lang="ts">
  import { ws } from "../lib/ws.svelte";
  import { formatStarted, runDuration, stateLabel } from "../lib/session-format";
  import type { SessionSummary } from "../lib/types";
  import CategoryBadge from "./CategoryBadge.svelte";
  import { Icon } from "../lib/icons";

  let {
    session,
    selected,
    onSelect,
  }: { session: SessionSummary; selected: boolean; onSelect: (runId: string) => void } = $props();

  let finished = $derived(session.state === "completed");
  let duration = $derived(runDuration(session, ws.sessions.now));
  let outcome = $derived(ws.sessions.outcomes.get(session.run_id));
  let error = $derived(ws.sessions.errors.get(session.run_id));

  let stateText = $derived.by(() => {
    if (!finished) return stateLabel(session.state);
    if (outcome?.status === "failed") return "failed";
    if (outcome?.status === "cancelled") return "stopped";
    if (session.interrupted) return "interrupted";
    return "finished";
  });
</script>

<li>
  <button
    type="button"
    class="session-row state-{session.state}"
    class:selected
    class:failed={outcome?.status === "failed"}
    aria-current={selected ? "true" : undefined}
    onclick={() => onSelect(session.run_id)}
  >
    <span class="session-row-seam" aria-hidden="true"></span>
    <span class="session-row-top">
      <CategoryBadge category={session.category} />
      <span class="session-row-source">{session.source_label}</span>
      <span class="session-row-time" title={finished ? "How long it ran" : "Running for"}>
        {duration}
      </span>
    </span>
    <span class="session-row-purpose">{session.purpose || session.address}</span>
    <span class="session-row-state">
      <span class="session-row-state-label">{stateText}</span>
      {#if finished}
        <span class="session-row-started">{formatStarted(session)}</span>
      {/if}
    </span>
    {#if error && !finished}
      <span class="session-row-error">
        <Icon name="warning" size={12} />
        <span class="session-row-error-text"
          ><span class="visually-hidden">Last error: </span>{error}</span
        >
      </span>
    {/if}
  </button>
</li>
