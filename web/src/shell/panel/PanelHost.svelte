<script lang="ts">
  import { untrack } from "svelte";
  import { LazyComponent } from "../../lib/lazy-component.svelte";
  import { overview } from "../../lib/overview.svelte";
  import { router } from "../../lib/router.svelte";
  import { SessionRun } from "../../lib/session-run.svelte";
  import { Skeleton } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import ContextPanel from "./ContextPanel.svelte";
  import ConversationSize from "../../places/chat/ConversationSize.svelte";
  import { NewSession } from "../../places/activity/new-session.svelte";
  import SessionPanel from "../../places/activity/SessionPanel.svelte";
  import { FileBuffer } from "../../places/files/file-buffer.svelte";
  import { fileSourceFor } from "../../places/files/file-source";

  // The context panel, open while the URL names a `panel` its place can show
  // (the router removes any other). Each kind's content renders inside the
  // frame and starts with a `PanelHeader`. The file view, with its editor,
  // loads the first time a file opens, and the new-session panel the first
  // time a session is started from Activity.

  const filePanel = new LazyComponent<{ buffer: FileBuffer; path: string }>(
    () => import("../../places/files/FilePanel.svelte"),
    "the file",
  );
  const newSessionPanel = new LazyComponent<{ session: NewSession }>(
    () => import("../../places/activity/NewSessionPanel.svelte"),
    "the new session",
  );

  const panel = $derived(router.panel);
  const fileSource = $derived(fileSourceFor(router.place));
  const fileTree = $derived(
    panel?.kind === "file" && fileSource !== null
      ? `${fileSource.scope}:${fileSource.agent ?? ""}`
      : null,
  );
  // The open file, the shown run and a new session live here, not in their
  // views: the frame draws its content again when the layout changes (a
  // phone's sheet, a column), and the edits, transcript, subscription and
  // task typed stay. A new one starts when the panel shows a file from
  // another tree, another run, or a new session on another agent.
  const fileBuffer = $derived.by(() => {
    if (fileTree === null) return null;
    return untrack(() => (fileSource === null ? null : new FileBuffer(fileSource)));
  });

  let shownRun: SessionRun | null = null;
  const sessionRun = $derived.by(() => {
    if (panel?.kind !== "session") return (shownRun = null);
    const { agent, runId } = panel;
    return untrack(() => {
      // The run following its session into a new run moved the URL along.
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

  // If a panel's code can't load, the panel closes so it can be asked for again.
  $effect(() => {
    if (panel?.kind === "file") filePanel.ensure(() => void router.closePanel());
    if (panel?.kind === "new-session") newSessionPanel.ensure(() => void router.closePanel());
  });

  $effect(() => {
    const run = sessionRun;
    if (run === null) return;
    // Opening reads the run's state; the run following a new one mustn't reopen it.
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
    {:else if panel.kind === "file"}
      {#if fileBuffer !== null}
        {#if filePanel.component !== null}
          {@const FilePanel = filePanel.component}
          {#key fileBuffer}
            <FilePanel buffer={fileBuffer} path={panel.path} />
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
