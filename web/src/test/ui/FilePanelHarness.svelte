<script lang="ts">
  import { untrack } from "svelte";
  import { FileBuffer } from "../../places/files/file-buffer.svelte";
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

  // Made once, from the props the test renders with.
  untrack(() => {
    providePanelFrame({ layout, titleId: "panel-title", close: onclose });
  });
  const buffer = untrack(() => new FileBuffer(source));
</script>

<aside aria-labelledby="panel-title">
  <FilePanel {buffer} {path} />
</aside>
