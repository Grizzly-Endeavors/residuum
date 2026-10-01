<script lang="ts">
  import { untrack } from "svelte";
  import { router } from "../../lib/router.svelte";
  import { Button, EmptyState } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import SessionView from "../../components/SessionView.svelte";
  import PanelHeader from "./PanelHeader.svelte";

  // A session run in the context panel, shown by the legacy session view. That
  // view reads the bound agent's sessions, so a run on another agent (from the
  // Workbench) offers to open it beside that agent's chat instead.

  let { agent, runId }: { agent: string; runId: string } = $props();

  const sessions = $derived(ws.sessions);
  const bound = $derived(ws.agent === agent);

  $effect(() => {
    if (!bound) return;
    const store = sessions;
    const run = runId;
    untrack(() => store.showRun(run));
  });

  // The run leaves the store's view when the panel closes or the agent's store is replaced.
  $effect(() => {
    const store = sessions;
    return () => store.closeView();
  });

  // A session that continues in a new run takes the panel with it.
  $effect(() => {
    const followed = sessions.view?.runId;
    if (followed === undefined) return;
    untrack(() => {
      if (followed !== runId) void router.replacePanel({ kind: "session", agent, runId: followed });
    });
  });
</script>

<PanelHeader icon="sessions" title="Session" />
{#if !bound}
  <div class="context-panel-elsewhere">
    <EmptyState variant="block" icon="sessions" title="This run is {agent}'s" headingLevel={3}>
      Open it beside {agent}'s chat to follow it, message it or stop it.
      {#snippet actions()}
        <Button
          onclick={() =>
            void router.openPlace(
              { kind: "chat", agent },
              { panel: { kind: "session", agent, runId } },
            )}
        >
          Open beside {agent}'s chat
        </Button>
      {/snippet}
    </EmptyState>
  </div>
{:else if sessions.view !== null}
  <div data-legacy-view>
    <SessionView view={sessions.view} />
  </div>
{/if}

<style>
  .context-panel-elsewhere {
    padding: 0 var(--space-18);
  }
</style>
