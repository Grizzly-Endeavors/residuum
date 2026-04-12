<script lang="ts">
  import type { ImageAttachment } from "../lib/types";

  let { content, images }: { content: string; images?: ImageAttachment[] } = $props();
</script>

<div class="msg msg-user">
  {#if content}
    <div class="msg-content">{content}</div>
  {/if}
  {#if images?.length}
    <div class="msg-user-images">
      {#each images as img, i (i)}
        <img src="data:{img.media_type};base64,{img.data}" alt="attachment" />
      {/each}
    </div>
  {/if}
</div>
