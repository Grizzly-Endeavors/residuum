<script lang="ts">
  import { untrack } from "svelte";
  import { FileBuffer } from "../../places/files/file-buffer.svelte";
  import FilePanel from "../../places/files/FilePanel.svelte";
  import type { FileSource } from "../../places/files/file-source";
  import { providePanelFrame, type PanelLayout } from "../../shell/panel/panel-frame";

  // The file panel inside a panel frame, as the context panel hosts it, so its
  // header finds one. A test can pass the buffer, to show a redrawn panel
  // with the file it already holds.
  let {
    source,
    path,
    agentNamed = false,
    layout = "wide",
    onclose = () => {},
    buffer: given,
  }: {
    source: FileSource;
    path: string;
    agentNamed?: boolean;
    layout?: PanelLayout;
    onclose?: () => void;
    buffer?: FileBuffer;
  } = $props();

  // Made once, from the props the test renders with.
  untrack(() => {
    providePanelFrame({ layout, titleId: "panel-title", close: onclose });
  });
  const buffer = untrack(() => given ?? new FileBuffer(source));
</script>

<aside aria-labelledby="panel-title">
  <FilePanel {buffer} {path} {agentNamed} />
</aside>
