<script lang="ts">
  import { untrack } from "svelte";
  import { router } from "../../lib/router.svelte";
  import ContextPanel from "./ContextPanel.svelte";
  import LegacySessionPanel from "./LegacySessionPanel.svelte";
  import LegacySizePanel from "./LegacySizePanel.svelte";
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
  // The open file lives here, not in its view: the frame draws its content
  // again when the layout changes (a phone's sheet, a column), and the edits
  // stay. A new one starts when the panel shows a file from another tree.
  const fileBuffer = $derived.by(() => {
    if (fileTree === null) return null;
    return untrack(() => (fileSource === null ? null : new FileBuffer(fileSource)));
  });
</script>

{#if panel !== null}
  <ContextPanel>
    {#if panel.kind === "session"}
      <LegacySessionPanel agent={panel.agent} runId={panel.runId} />
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
