<script lang="ts">
  import { renderMarkdown } from "../lib/markdown";

  let { content }: { content: string } = $props();
</script>

<!-- eslint-disable-next-line svelte/no-at-html-tags -- renderMarkdown sanitizes its output with DOMPurify -->
{@html renderMarkdown(content)}
