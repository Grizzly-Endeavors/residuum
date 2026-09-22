<script lang="ts">
  import MarkdownContent from "./MarkdownContent.svelte";

  let { content }: { content: string } = $props();
</script>

<div class="msg msg-assistant">
  <div class="msg-content"><MarkdownContent {content} /></div>
</div>
