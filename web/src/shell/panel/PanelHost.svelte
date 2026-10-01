<script lang="ts">
  import { untrack } from "svelte";
  import { overview } from "../../lib/overview.svelte";
  import { router } from "../../lib/router.svelte";
  import { SessionRun } from "../../lib/session-run.svelte";
  import { ws } from "../../lib/ws.svelte";
  import ContextPanel from "./ContextPanel.svelte";
  import LegacySizePanel from "./LegacySizePanel.svelte";
  import SessionPanel from "../../places/activity/SessionPanel.svelte";
  import FilePanel from "../../places/files/FilePanel.svelte";
  import { FileBuffer } from "../../places/files/file-buffer.svelte";
  import { fileSourceFor } from "../../places/files/file-source";

  // The context panel, open while the URL names a `panel` its place can show
  // (the router removes any other). Each kind's content renders inside the
  // frame and starts with a `PanelHeader`; a kind that hasn't been rebuilt
  // hosts its legacy view.

  const panel = $derived(router.panel);
  const fileSource = $derived(fileSourceFor(router.place));
  const fileTree = $derived(
    panel?.kind === "file" && fileSource !== null
      ? `${fileSource.scope}:${fileSource.agent ?? ""}`
      : null,
  );
  // The open file and the shown run live here, not in their views: the frame
  // draws its content again when the layout changes (a phone's sheet, a
  // column), and the edits, transcript and subscription stay. A new one
  // starts when the panel shows a file from another tree, or another run.
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

  $effect(() => {
    const run = sessionRun;
    if (run === null) return;
    return run.open();
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
    {:else if panel.kind === "file"}
      {#if fileBuffer !== null}
        {#key fileBuffer}
          <FilePanel buffer={fileBuffer} path={panel.path} />
        {/key}
      {/if}
    {:else}
      <LegacySizePanel />
    {/if}
  </ContextPanel>
{/if}
