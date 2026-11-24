<script lang="ts">
  import { untrack } from "svelte";
  import { LazyComponent } from "../../lib/lazy-component.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { router } from "../../lib/router.svelte";
  import { SessionRun } from "../../lib/session-run.svelte";
  import { Skeleton } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import ContextPanel from "./ContextPanel.svelte";
  import Activity from "../../places/activity/Activity.svelte";
  import ConversationSize from "../../places/chat/ConversationSize.svelte";
  import { NewSession } from "../../places/activity/new-session.svelte";
  import SessionPanel from "../../places/activity/SessionPanel.svelte";
  import FilesPlace from "../../places/files/FilesPlace.svelte";
  import { FileBuffer } from "../../places/files/file-buffer.svelte";
  import { panelFileSource } from "../../places/files/file-source";
  import Schedule from "../../places/schedule/Schedule.svelte";

  const filePanel = new LazyComponent<{ buffer: FileBuffer; path: string; agentNamed: boolean }>(
    () => import("../../places/files/FilePanel.svelte"),
    "the file",
  );
  const newSessionPanel = new LazyComponent<{ session: NewSession }>(
    () => import("../../places/activity/NewSessionPanel.svelte"),
    "the new session",
  );

  const panel = $derived(router.panel);
  const fileSource = $derived(panel?.kind === "file" ? panelFileSource(router.place, panel) : null);
  const fileTree = $derived(
    panel?.kind === "file" && fileSource !== null
      ? `${fileSource.scope}:${fileSource.agent ?? ""}`
      : null,
  );
  const fileBuffer = $derived.by(() => {
    if (fileTree === null) return null;
    return untrack(() => (fileSource === null ? null : new FileBuffer(fileSource)));
  });

  let shownRun: SessionRun | null = null;
  const sessionRun = $derived.by(() => {
    if (panel?.kind !== "session") return (shownRun = null);
    const { agent, runId } = panel;
    return untrack(() => {
      if (shownRun?.agent === agent && shownRun.runId === runId) return shownRun;
      const known = ws.agent === agent ? ws.sessions.findRun(runId) : undefined;
      const live = overview.overviewOf(agent)?.live_sessions.find((s) => s.run_id === runId);
      shownRun = new SessionRun(agent, runId, {
        summary: known === undefined ? null : $state.snapshot(known),
        address: live?.address ?? null,
      });
      return shownRun;
    });
  });

  let shownNewSession: NewSession | null = null;
  const newSession = $derived.by(() => {
    if (panel?.kind !== "new-session") return (shownNewSession = null);
    const { agent } = panel;
    return untrack(() => {
      if (shownNewSession?.agent !== agent) shownNewSession = new NewSession(agent);
      return shownNewSession;
    });
  });

  $effect(() => {
    if (panel?.kind === "file") filePanel.ensure(() => void router.closePanel());
    if (panel?.kind === "new-session") newSessionPanel.ensure(() => void router.closePanel());
  });

  $effect(() => {
    const run = sessionRun;
    if (run === null) return;
    return untrack(() => run.open());
  });

  $effect(() => {
    const run = sessionRun;
    if (run === null) return;
    const runId = run.runId;
    untrack(() => {
      if (panel?.kind === "session" && panel.runId !== runId) {
        void router.replacePanel({ kind: "session", agent: run.agent, runId });
      }
    });
  });
</script>

{#if panel !== null}
  <ContextPanel>
    {#if panel.kind === "session"}
      {#if sessionRun !== null}
        <SessionPanel run={sessionRun} />
      {/if}
    {:else if panel.kind === "new-session"}
      {#if newSession !== null}
        {#if newSessionPanel.component !== null}
          {@const NewSessionPanel = newSessionPanel.component}
          <NewSessionPanel session={newSession} />
        {:else}
          <div class="panel-loading"><Skeleton lines={4} label="Loading the new session" /></div>
        {/if}
      {/if}
    {:else if panel.kind === "activity"}
      {#key panel.agent}
        <Activity agent={panel.agent} inPanel />
      {/key}
    {:else if panel.kind === "schedule"}
      {#key panel.agent}
        <Schedule agent={panel.agent} inPanel />
      {/key}
    {:else if panel.kind === "files"}
      {#key panel.agent}
        <FilesPlace source={{ agent: panel.agent, scope: "agent" }} inPanel />
      {/key}
    {:else if panel.kind === "file"}
      {#if fileBuffer !== null}
        {#if filePanel.component !== null}
          {@const FilePanel = filePanel.component}
          {#key fileBuffer}
            <FilePanel buffer={fileBuffer} path={panel.path} agentNamed={panel.agent !== null} />
          {/key}
        {:else}
          <div class="panel-loading"><Skeleton lines={8} label="Loading the file" /></div>
        {/if}
      {/if}
    {:else}
      <ConversationSize />
    {/if}
  </ContextPanel>
{/if}

<style>
  .panel-loading {
    padding: var(--space-16) var(--space-18);
  }
</style>
