<script lang="ts">
  import { IconButton, ModalLayer, VisuallyHidden } from "../lib/ui";
  import type { ViewerImage } from "./image-viewer";

  // A picture from the conversation at full size, in a modal layer: it fits the
  // view with its alt text above it. With more than one picture (a message's
  // images), the arrow keys and the buttons beside the title move between
  // them, wrapping at either end, and the layer says which one is showing.
  // `index` is the picture showing, or null while closed.

  interface Props {
    images: readonly ViewerImage[];
    index: number | null;
  }

  let { images, index = $bindable(null) }: Props = $props();

  const uid = $props.id();
  const current = $derived(index === null ? undefined : images[index]);
  const many = $derived(images.length > 1);
  const place = $derived(index === null ? "" : `${String(index + 1)} of ${String(images.length)}`);

  function step(by: number): void {
    if (index === null) return;
    index = (index + by + images.length) % images.length;
  }

  function onkeydown(event: KeyboardEvent): void {
    if (current === undefined || !many || event.defaultPrevented) return;
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault();
    step(event.key === "ArrowRight" ? 1 : -1);
  }
</script>

<!-- The viewer is modal, so no other control is listening for the arrow keys while it is open. -->
<svelte:document {onkeydown} />

<ModalLayer
  open={current !== undefined}
  frame="viewer"
  width={many ? "min(100%, 1280px)" : "fit-content"}
  labelledby="{uid}-title"
  initialFocus={false}
  onclose={() => (index = null)}
>
  {#if current}
    <header class="viewer-head">
      <h2 id="{uid}-title" class="viewer-title">{current.alt}</h2>
      {#if many}
        <span class="viewer-place" aria-hidden="true">{place}</span>
        <IconButton icon="chevron-left" label="Previous image" size="sm" onclick={() => step(-1)} />
        <IconButton icon="chevron-right" label="Next image" size="sm" onclick={() => step(1)} />
      {/if}
      <IconButton
        icon="close"
        label="Close"
        size="sm"
        data-overlay-close
        onclick={() => (index = null)}
      />
    </header>
    <div class="viewer-stage" data-fixed={many ? "" : undefined}>
      <img class="viewer-image" src={current.src} alt={current.alt} draggable="false" />
    </div>
    {#if many}
      <VisuallyHidden><span role="status">{place}: {current.alt}</span></VisuallyHidden>
    {/if}
  {/if}
</ModalLayer>

<style>
  .viewer-head {
    display: flex;
    flex: none;
    align-items: center;
    gap: var(--space-4);
    min-width: min(22rem, 100%);
    height: var(--space-48);
    padding: 0 var(--space-10) 0 var(--space-16);
  }

  .viewer-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-regular);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .viewer-place {
    flex: none;
    margin-right: var(--space-4);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  /* A well the picture sits in, so a small one still has a card around it. The view less the layer's margin and the head is the most it can have. */
  .viewer-stage {
    --viewer-room: calc(
      100dvh - var(--space-32) - var(--space-48) - var(--safe-top) - var(--safe-bottom)
    );

    display: flex;
    flex: 1 1 auto;
    align-items: center;
    justify-content: center;
    min-height: 0;
    background: var(--color-stone-2);
  }

  /* A set of pictures keeps one frame, so the buttons stay where they are as the pictures change. */
  .viewer-stage[data-fixed] {
    height: var(--viewer-room);
  }

  .viewer-image {
    display: block;
    max-width: 100%;
    max-height: var(--viewer-room);
    object-fit: contain;
  }
</style>
