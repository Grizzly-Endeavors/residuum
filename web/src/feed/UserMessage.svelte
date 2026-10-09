<script lang="ts">
  import { userErrorMessage } from "../lib/errors";
  import { notifications } from "../lib/notifications.svelte";
  import { turnChangedWorkspace, undoTurn } from "../lib/turn-undo";
  import type { UserFeedItem } from "../lib/types";
  import { Button } from "../lib/ui";
  import Timestamp from "./Timestamp.svelte";

  // The user's message: a moss bubble on the right, with a sender line when
  // it came from another interface or a workbench artifact, and its time under
  // it. A turn observed live that changed files offers Undo this turn, in
  // `agent`'s workspace.

  let { item, agent }: { item: UserFeedItem; agent: string } = $props();

  const senderLine = $derived(
    item.sender
      ? [item.sender.name, item.sender.interface, item.sender.location].filter(Boolean).join(" · ")
      : null,
  );

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
  <div class="user-bubble">
    {#if item.content}
      <p class="user-text">{item.content}</p>
    {/if}
    {#if item.images?.length}
      <div class="user-images">
        {#each item.images as image, index (index)}
          <img src="data:{image.media_type};base64,{image.data}" alt="Attached image {index + 1}" />
        {/each}
      </div>
    {/if}
  </div>
  {#if item.turn?.changed === true || item.timestamp !== undefined}
    <div class="user-foot" data-message-meta>
      {#if item.turn?.changed}
        <Button
          variant="quiet"
          size="sm"
          icon="restore"
          loading={undoing}
          onclick={() => void undo()}
        >
          Undo this turn
        </Button>
      {/if}
      {#if item.timestamp !== undefined}
        <Timestamp timestamp={item.timestamp} />
      {/if}
    </div>
  {/if}
</div>

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
      max-width: 160px;
      max-height: 160px;
      border-radius: var(--corner-md);
      object-fit: cover;
    }
  }

  /* Under the bubble: Undo this turn, and when it was sent. The row hangs into the gap below it, so the time takes little height. */
  .user-foot {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-8);
    min-height: var(--space-20);
    margin-bottom: calc(-1 * var(--space-12));
  }

  @media (max-width: 760px) {
    .user-bubble {
      max-width: 90%;
    }
  }
</style>
