<script lang="ts">
  import type { SessionRun } from "../../lib/session-run.svelte";
  import SessionPanel from "../../places/activity/SessionPanel.svelte";
  import { providePanelFrame } from "../../shell/panel/panel-frame";

  let { run }: { run: SessionRun } = $props();

  providePanelFrame({ layout: "wide", titleId: "panel-title", close: () => {} });
</script>

<aside aria-labelledby="panel-title">
  <SessionPanel {run} />
</aside>
