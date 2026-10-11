<script lang="ts">
  import ConversationSize from "../../places/chat/ConversationSize.svelte";
  import { providePanelFrame } from "../../shell/panel/panel-frame";

  // The conversation size inside a panel frame, as the context panel hosts it,
  // so its header finds one.

  providePanelFrame({ layout: "wide", titleId: "panel-title", close: () => {} });
</script>

<aside aria-labelledby="panel-title">
  <ConversationSize />
</aside>
