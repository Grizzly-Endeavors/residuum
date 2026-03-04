<script lang="ts">
  import { escapeHtml } from "../lib/markdown";

  let { content }: { content: string } = $props();
</script>

<div class="msg msg-user">
  <div class="msg-content">{@html escapeHtml(content)}</div>
</div>
