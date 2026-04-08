<script lang="ts">
  import type { FileAttachmentFeedItem } from "../lib/types";

  let { item }: { item: FileAttachmentFeedItem } = $props();

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  const isImage = $derived(item.mimeType.startsWith("image/"));
  const isAudio = $derived(item.mimeType.startsWith("audio/"));
</script>

<div class="msg msg-assistant file-attachment">
  {#if item.caption}
    <p class="file-attachment-caption">{item.caption}</p>
  {/if}

  {#if isImage}
    <img src={item.url} alt={item.filename} class="file-attachment-image" />
  {:else if isAudio}
    <audio controls src={item.url} class="file-attachment-audio">
      <track kind="captions" />
    </audio>
  {:else}
    <a href={item.url} download={item.filename} class="file-attachment-download">
      {item.filename} ({formatSize(item.size)})
    </a>
  {/if}
</div>
