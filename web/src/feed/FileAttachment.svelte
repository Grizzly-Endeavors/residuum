<script lang="ts">
  import { Icon } from "../lib/icons";
  import type { FileAttachmentFeedItem } from "../lib/types";
  import { fileSize } from "../lib/file-size";
  import ImageViewer from "./ImageViewer.svelte";
  import { viewImageLabel, type ViewerImage } from "./image-viewer";
  import Timestamp from "./Timestamp.svelte";

  // A file the agent sent: its caption, the image or audio inline when it is
  // one (an image opens full size), and always the file to download with its
  // name and size, and when it was sent beside it.

  let { item }: { item: FileAttachmentFeedItem } = $props();

  const isImage = $derived(item.mimeType.startsWith("image/"));
  const isAudio = $derived(item.mimeType.startsWith("audio/"));
  const picture = $derived<ViewerImage>({ src: item.url, alt: item.caption ?? item.filename });
  let viewing = $state<number | null>(null);
</script>

<div class="attachment">
  {#if item.caption}
    <p class="attachment-caption">{item.caption}</p>
  {/if}
  {#if isImage}
    <button
      type="button"
      class="attachment-view"
      aria-haspopup="dialog"
      aria-label={viewImageLabel(picture.alt)}
      onclick={() => (viewing = 0)}
    >
      <img class="attachment-image" src={picture.src} alt={picture.alt} />
    </button>
    <ImageViewer images={[picture]} bind:index={viewing} />
  {:else if isAudio}
    <audio class="attachment-audio" controls src={item.url} aria-label={item.filename}>
      <track kind="captions" />
    </audio>
  {/if}
  <div class="attachment-foot">
    <a class="attachment-file" href={item.url} download={item.filename}>
      <Icon name="paperclip" size={14} />
      <span class="attachment-name">{item.filename}</span>
      <span class="attachment-size">{fileSize(item.size)}</span>
    </a>
    {#if item.timestamp !== undefined}
      <Timestamp timestamp={item.timestamp} />
    {/if}
  </div>
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

  /* The image is a button that opens it full size. */
  .attachment-view {
    display: block;
    max-width: 100%;
    padding: 0;
    border-radius: var(--corner-md);
    cursor: zoom-in;
    line-height: 0;
  }

  .attachment-image {
    display: block;
    max-width: min(100%, 480px);
    max-height: 360px;
    border-radius: inherit;
  }

  .attachment-audio {
    width: min(100%, 360px);
  }

  .attachment-foot {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    max-width: 100%;
    min-width: 0;
  }

  .attachment-file {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    min-width: 0;
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
