<script lang="ts">
  import { Icon } from "../lib/icons";
  import type { FileAttachmentFeedItem } from "../lib/types";
  import { fileSize } from "../lib/file-size";

  // A file the agent sent: its caption, the image or audio inline when it is
  // one, and always the file to download with its name and size.

  let { item }: { item: FileAttachmentFeedItem } = $props();

  const isImage = $derived(item.mimeType.startsWith("image/"));
  const isAudio = $derived(item.mimeType.startsWith("audio/"));
</script>

<div class="attachment">
  {#if item.caption}
    <p class="attachment-caption">{item.caption}</p>
  {/if}
  {#if isImage}
    <img class="attachment-image" src={item.url} alt={item.caption ?? item.filename} />
  {:else if isAudio}
    <audio class="attachment-audio" controls src={item.url} aria-label={item.filename}>
      <track kind="captions" />
    </audio>
  {/if}
  <a class="attachment-file" href={item.url} download={item.filename}>
    <Icon name="paperclip" size={14} />
    <span class="attachment-name">{item.filename}</span>
    <span class="attachment-size">{fileSize(item.size)}</span>
  </a>
</div>

<style>
  .attachment {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-8);
    min-width: 0;
  }

  .attachment-caption {
    font-size: var(--font-size-message);
    line-height: var(--line-height-message);
  }

  .attachment-image {
    max-width: min(100%, 480px);
    max-height: 360px;
    border-radius: var(--corner-md);
  }

  .attachment-audio {
    width: min(100%, 360px);
  }

  .attachment-file {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    max-width: 100%;
    min-height: 32px;
    padding: 0 var(--space-12) 0 var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-stone-1);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    text-decoration: none;
    transition: background var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-3);
    }

    & :global(svg) {
      flex: none;
      color: var(--color-text-2);
    }
  }

  .attachment-name {
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .attachment-size {
    flex: none;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  @media (max-width: 760px) {
    .attachment-file {
      min-height: var(--layout-touch-target);
    }
  }
</style>
