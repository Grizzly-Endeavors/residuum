<script lang="ts">
  import { router } from "../../lib/router.svelte";
  import ContextPanel from "./ContextPanel.svelte";
  import LegacySessionPanel from "./LegacySessionPanel.svelte";
  import LegacySizePanel from "./LegacySizePanel.svelte";
  import FilePanel from "../../places/files/FilePanel.svelte";
  import { fileSourceFor } from "../../places/files/file-source";

  // The context panel, open while the URL names a `panel` its place can show
  // (the router removes any other). Each kind's content renders inside the
  // frame and starts with a `PanelHeader`; a kind that hasn't been rebuilt
  // hosts its legacy view.

  const panel = $derived(router.panel);
  const fileSource = $derived(fileSourceFor(router.place));
</script>

{#if panel !== null}
  <ContextPanel>
    {#if panel.kind === "session"}
      <LegacySessionPanel agent={panel.agent} runId={panel.runId} />
    {:else if panel.kind === "file"}
      {#if fileSource !== null}
        {#key `${fileSource.scope}:${fileSource.agent ?? ""}`}
          <FilePanel source={fileSource} path={panel.path} />
        {/key}
      {/if}
    {:else}
      <LegacySizePanel />
    {/if}
  </ContextPanel>
{/if}
