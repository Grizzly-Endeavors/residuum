<script lang="ts">
  import { userErrorMessage } from "../lib/errors";
  import { notifications } from "../lib/notifications.svelte";
  import { turnChangedWorkspace, undoTurn } from "../lib/turn-undo";
  import type { UserFeedItem } from "../lib/types";
  import { IconButton } from "../lib/ui";
  import ImageViewer from "./ImageViewer.svelte";
  import { viewImageLabel, type ViewerImage } from "./image-viewer";
  import Timestamp from "./Timestamp.svelte";

  // The user's message: a moss bubble on the right, with a sender line when
  // it came from another interface or a workbench artifact. Its time sits left
  // of the bubble, taking no room below it; where the conversation is too
  // narrow for that, it floats over the gap under the bubble. Its images open
  // full size. A turn observed live that changed files offers Undo this turn,
  // in `agent`'s workspace, always in sight beside the bubble.

  let { item, agent }: { item: UserFeedItem; agent: string } = $props();

  const senderLine = $derived(
    item.sender
      ? [item.sender.name, item.sender.interface, item.sender.location].filter(Boolean).join(" · ")
      : null,
  );

  // The images the message carried, each opening full size in the viewer.
  const pictures = $derived<ViewerImage[]>(
    (item.images ?? []).map((image, index) => ({
      src: `data:${image.media_type};base64,${image.data}`,
      alt: `Attached image ${String(index + 1)}`,
    })),
  );
  let viewing = $state<number | null>(null);

  let undoing = $state(false);

  // Whether the turn changed anything is asked once, when it ends. If that
  // can't be told, Undo stays hidden rather than offering nothing to undo.
  $effect(() => {
    const turn = item.turn;
    if (turn?.changed !== null) return;
    turnChangedWorkspace(agent, turn.turnId)
      .then((changed) => {
        turn.changed = changed;
      })
      .catch(() => {
        turn.changed = false;
      });
  });

  function files(count: number): string {
    return `${count} file${count === 1 ? "" : "s"}`;
  }

  async function undo(): Promise<void> {
    const turn = item.turn;
    if (turn === undefined) return;
    undoing = true;
    try {
      const outcome = await undoTurn(agent, turn.turnId);
      if (outcome === null) {
        notifications.surface("error", "Couldn't undo this turn: no checkpoint was found for it.");
        return;
      }
      const parts = [`Undid this turn: put back ${files(outcome.reverted_paths.length)}.`];
      const skipped = outcome.skipped_paths;
      if (skipped.length > 0) {
        const were = skipped.length === 1 ? "it was" : "they were";
        parts.push(`Left ${skipped.join(", ")} alone: ${were} changed again after this turn.`);
      }
      notifications.surface("notice", parts.join(" "));
      turn.changed = false;
    } catch (err: unknown) {
      notifications.surface("error", userErrorMessage(err, { action: "Couldn't undo this turn." }));
    } finally {
      undoing = false;
    }
  }
</script>

<div class="user-message">
  {#if senderLine}
    <p class="user-sender">{senderLine}</p>
  {/if}
  <div class="user-row">
    {#if item.turn?.changed}
      <IconButton
        size="sm"
        icon="restore"
        label="Undo this turn"
        loading={undoing}
        onclick={() => void undo()}
      />
    {/if}
    {#if item.timestamp !== undefined}
      <span class="user-time"><Timestamp timestamp={item.timestamp} /></span>
    {/if}
    <div class="user-bubble">
      {#if item.content}
        <p class="user-text">{item.content}</p>
      {/if}
      {#if pictures.length > 0}
        <div class="user-images">
          {#each pictures as picture, index (index)}
            <button
              type="button"
              class="user-image"
              aria-haspopup="dialog"
              aria-label={viewImageLabel(picture.alt)}
              onclick={() => (viewing = index)}
            >
              <img src={picture.src} alt={picture.alt} />
            </button>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

<ImageViewer images={pictures} bind:index={viewing} />

<style>
  .user-message {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-4);
  }

  .user-sender {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .user-bubble {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    flex: 0 1 auto;
    min-width: 0;
    max-width: min(82%, 560px);
    padding: var(--space-8) var(--space-14);
    border-radius: var(--corner-lg) var(--corner-lg) var(--corner-sm) var(--corner-lg);
    background: var(--color-moss-tint);
    color: var(--color-text);
  }

  .user-text {
    font-size: var(--font-size-message);
    line-height: var(--line-height-ui);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .user-images {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);

    & img {
      display: block;
      max-width: 160px;
      max-height: 160px;
      border-radius: inherit;
      object-fit: cover;
    }
  }

  /* A picture is a button that opens it full size. */
  .user-image {
    display: block;
    padding: 0;
    border-radius: var(--corner-md);
    cursor: zoom-in;
    line-height: 0;
  }

  /* Undo and the time line up left of the bubble, on its last line. */
  .user-row {
    position: relative;
    display: flex;
    align-items: flex-end;
    align-self: stretch;
    justify-content: flex-end;
    gap: var(--space-8);
  }

  /* On the baseline of the bubble's last line. */
  .user-time {
    display: flex;
    align-self: last baseline;
  }

  /* Too narrow to spare the room beside the bubble: the time floats over the gap under it. */
  @container feed (max-width: 639px) {
    .user-time {
      position: absolute;
      top: calc(100% + var(--space-2));
      right: 0;
      z-index: var(--z-sticky);
      padding: var(--space-4) var(--space-10);
      border-radius: var(--corner-pill);
      background: var(--color-stone-3);
      box-shadow: var(--shadow-float);
      opacity: var(--message-meta, 1);
      pointer-events: var(--message-meta-events, auto);
      transition: opacity var(--duration-fast) var(--ease-out);
    }
  }

  @media (max-width: 760px) {
    .user-bubble {
      max-width: 90%;
    }
  }
</style>
