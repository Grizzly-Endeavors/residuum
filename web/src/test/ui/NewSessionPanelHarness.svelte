<script lang="ts">
  import NewSessionPanel from "../../places/activity/NewSessionPanel.svelte";
  import type { NewSession } from "../../places/activity/new-session.svelte";
  import { providePanelFrame } from "../../shell/panel/panel-frame";

  // The new-session panel inside a panel frame, as the context panel hosts
  // it, so its header finds one.
  let { session }: { session: NewSession } = $props();

  providePanelFrame({ layout: "wide", titleId: "panel-title", close: () => {} });
</script>

<aside aria-labelledby="panel-title">
  <NewSessionPanel {session} />
</aside>
