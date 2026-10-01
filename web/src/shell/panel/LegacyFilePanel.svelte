<script lang="ts">
  import WorkspaceEditor from "../../components/WorkspaceEditor.svelte";
  import { fileName, type FileSource } from "./panel-file";
  import PanelHeader from "./PanelHeader.svelte";

  // A file in the context panel, in the legacy workspace editor.

  let { source, path }: { source: FileSource; path: string } = $props();

  let editor = $state<WorkspaceEditor>();
  const inFolder = $derived(fileName(path) !== path);

  $effect(() => {
    void editor?.open(path);
  });
</script>

<PanelHeader
  icon="file"
  kind="File"
  title={fileName(path)}
  code
  meta={inFolder ? folderPath : undefined}
/>
{#key `${source.scope}:${source.agent ?? ""}`}
  <div data-legacy-view>
    <WorkspaceEditor bind:this={editor} agent={source.agent} scope={source.scope} header={false} />
  </div>
{/key}

{#snippet folderPath()}
  <code class="context-panel-path">{path}</code>
{/snippet}

<style>
  .context-panel-path {
    font-size: var(--font-size-xs);
  }
</style>
