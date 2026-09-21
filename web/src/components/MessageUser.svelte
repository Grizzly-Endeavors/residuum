<script lang="ts">
  import type { ImageAttachment, MessageSender } from "../lib/types";

  let {
    content,
    images,
    sender,
  }: { content: string; images?: ImageAttachment[]; sender?: MessageSender } = $props();

  let senderLabel = $derived(
    sender ? [sender.name, sender.interface, sender.location].filter(Boolean).join(" · ") : null,
  );
</script>

<div class="msg msg-user">
  {#if senderLabel}
    <div class="msg-user-sender">{senderLabel}</div>
  {/if}
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
