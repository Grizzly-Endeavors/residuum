<script lang="ts">
  import type { SessionRun } from "../../lib/session-run.svelte";
  import SessionPanel from "../../places/activity/SessionPanel.svelte";
  import { providePanelFrame } from "../../shell/panel/panel-frame";

  // The session panel inside a panel frame, as the context panel hosts it, so
  // its header finds one.
  let { run }: { run: SessionRun } = $props();

  providePanelFrame({ layout: "wide", titleId: "panel-title", close: () => {} });
</script>

<aside aria-labelledby="panel-title">
  <SessionPanel {run} />
</aside>
