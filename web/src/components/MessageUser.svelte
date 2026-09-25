<script lang="ts">
  import { turnChangedWorkspace, undoTurn } from "../lib/turn-undo";
  import { userErrorMessage } from "../lib/errors";
  import { toast } from "../lib/toast.svelte";
  import type { ImageAttachment, MessageSender, TurnRef } from "../lib/types";

  let {
    content,
    images,
    sender,
    turn,
  }: {
    content: string;
    images?: ImageAttachment[];
    sender?: MessageSender;
    turn?: TurnRef;
  } = $props();

  let senderLabel = $derived(
    sender ? [sender.name, sender.interface, sender.location].filter(Boolean).join(" · ") : null,
  );

  let undoing = $state(false);

  // Check, once, whether this turn is worth offering an undo for. Runs
  // only for a turn observed live (see `TurnRef`'s doc comment) — a no-op
  // once `turn.changed` is known.
  $effect(() => {
    if (turn?.changed !== null) return;
    const ref = turn;
    void turnChangedWorkspace(ref.turnId)
      .then((changed) => {
        ref.changed = changed;
      })
      .catch(() => {
        // Couldn't tell — default to hiding the control rather than
        // offering an undo that might not resolve to anything.
        ref.changed = false;
      });
  });

  async function handleUndo(): Promise<void> {
    if (!turn) return;
    undoing = true;
    try {
      const outcome = await undoTurn(turn.turnId);
      if (!outcome) {
        toast.error("Couldn't undo this turn — no checkpoint was found for it.");
        return;
      }
      const parts = [`Reverted ${outcome.reverted_paths.length} path(s) from this turn.`];
      if (outcome.skipped_paths.length > 0) {
        parts.push(`Skipped (changed again since): ${outcome.skipped_paths.join(", ")}.`);
      }
      toast.success(parts.join(" "));
      turn.changed = false;
    } catch (err: unknown) {
      toast.error(userErrorMessage(err, { action: "Couldn't undo this turn." }));
    } finally {
      undoing = false;
    }
  }
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
  {#if turn?.changed}
    <button
      type="button"
      class="msg-user-undo-turn"
      disabled={undoing}
      onclick={() => void handleUndo()}
      title="Revert every file this turn changed back to before it started"
    >
      {undoing ? "Undoing…" : "Undo this turn"}
    </button>
  {/if}
</div>

<style>
  .msg-user-undo-turn {
    margin-top: var(--s-2);
    padding: 2px var(--s-2);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-dim);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition:
      color var(--dur-quick),
      border-color var(--dur-quick),
      background var(--dur-quick);
  }

  .msg-user-undo-turn:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--vein-dim);
    background: var(--vein-faint);
  }

  .msg-user-undo-turn:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
