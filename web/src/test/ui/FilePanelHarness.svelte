<script lang="ts">
  import FilePanel from "../../places/files/FilePanel.svelte";
  import type { FileSource } from "../../places/files/file-source";
  import { providePanelFrame, type PanelLayout } from "../../shell/panel/panel-frame";

  // The file panel inside a panel frame, as the context panel hosts it, so its
  // header finds one.
  let {
    source,
    path,
    layout = "wide",
    onclose = () => {},
  }: { source: FileSource; path: string; layout?: PanelLayout; onclose?: () => void } = $props();

  // svelte-ignore state_referenced_locally
  providePanelFrame({ layout, titleId: "panel-title", close: onclose });
</script>

<aside aria-labelledby="panel-title">
  <FilePanel {source} {path} />
</aside>
