<script lang="ts">
  import { ws } from "../lib/ws.svelte";
  import { router } from "../lib/router.svelte";
  import {
    formatStarted,
    isStoppableState,
    runDuration,
    sessionArtifact,
    sessionSourceText,
    stateLabel,
  } from "../lib/session-format";
  import type { SessionSummary } from "../lib/types";
  import CategoryBadge from "./CategoryBadge.svelte";
  import { Icon } from "../lib/icons";

  let {
    session,
    selected,
    onSelect,
  }: { session: SessionSummary; selected: boolean; onSelect: (runId: string) => void } = $props();

  let finished = $derived(session.state === "completed");
  let artifact = $derived(sessionArtifact(session));
  let duration = $derived(runDuration(session, ws.sessions.now));
  let outcome = $derived(ws.sessions.outcomes.get(session.run_id));
  let error = $derived(ws.sessions.errors.get(session.run_id));
  let canStop = $derived(isStoppableState(session.state));
  let stopping = $derived(ws.sessions.stopping.has(session.address));

  let stateText = $derived.by(() => {
    if (!finished) return stateLabel(session.state);
    if (outcome?.status === "failed") return "failed";
    if (outcome?.status === "cancelled") return "stopped";
    if (session.interrupted) return "interrupted";
    return "finished";
  });
</script>

<li>
  <div
    class="session-row state-{session.state}"
    class:selected
    class:failed={outcome?.status === "failed"}
  >
    <button
      type="button"
      class="session-row-select"
      aria-current={selected ? "true" : undefined}
      onclick={() => onSelect(session.run_id)}
    >
      <span class="visually-hidden"
        >{sessionSourceText(session)}. {session.purpose || session.address}. {stateText}.</span
      >
    </button>
    <span class="session-row-seam" aria-hidden="true"></span>
    <span class="session-row-top">
      <CategoryBadge category={session.category} />
      {#if artifact}
        <button
          type="button"
          class="session-row-source session-row-artifact-link"
          title="Open the artifact {artifact}"
          onclick={() => router.openWorkbench(artifact)}
        >
          {sessionSourceText(session)}
        </button>
      {:else}
        <span class="session-row-source">{sessionSourceText(session)}</span>
      {/if}
      <span class="session-row-time" title={finished ? "How long it ran" : "Running for"}>
        {duration}
      </span>
      {#if canStop}
        <button
          type="button"
          class="session-row-stop"
          title={stopping ? "Stopping…" : "Stop this session"}
          aria-label={stopping ? `Stopping ${session.address}` : `Stop ${session.address}`}
          disabled={stopping}
          onclick={() => ws.sessions.stop(session.address)}
        >
          <Icon name="stop" size={10} />
        </button>
      {/if}
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
  </div>
</li>
