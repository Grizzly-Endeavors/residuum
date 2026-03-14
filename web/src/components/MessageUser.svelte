<script lang="ts">
  import { escapeHtml } from "../lib/markdown";
  import type { ImageAttachment } from "../lib/types";

  let { content, images }: { content: string; images?: ImageAttachment[] } = $props();
</script>

<div class="msg msg-user">
  <!-- eslint-disable svelte/no-at-html-tags -->
  {#if content}
    <div class="msg-content">{@html escapeHtml(content)}</div>
  {/if}
  {#if images?.length}
    <div class="msg-user-images">
      {#each images as img}
        <img src="data:{img.media_type};base64,{img.data}" alt="attachment" />
      {/each}
    </div>
  {/if}
</div>
