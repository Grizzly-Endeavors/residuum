<script lang="ts">
  import { renderMarkdown } from "../lib/markdown";

  let { content }: { content: string } = $props();
</script>

<div class="msg msg-assistant">
  <div class="msg-content">{@html renderMarkdown(content)}</div>
</div>
