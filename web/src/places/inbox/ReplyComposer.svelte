<script lang="ts">
  import { mail, type ReplyDraft } from "../../lib/mail.svelte";
  import { Icon } from "../../lib/icons";
  import { toast } from "../../lib/toast.svelte";
  import { Button, TextField } from "../../lib/ui";
  import AttachFiles from "./AttachFiles.svelte";
  import ComposerHint from "./ComposerHint.svelte";
  import StagedFiles from "../../composer/StagedFiles.svelte";
  import { joinNames } from "../../lib/words";
  import { deliveryHint, sendOnModEnter } from "./thread-model";

  // The reply at the foot of an open thread, to everyone in it. The text and
  // files live in the store's draft for the thread, so they survive moving
  // around the Inbox; a send waits for uploads still running.

  interface Props {
    draft: ReplyDraft;
    recipients: string[];
  }

  let { draft, recipients }: Props = $props();

  const blank = $derived(draft.body.trim() === "");

  async function send(event?: SubmitEvent): Promise<void> {
    event?.preventDefault();
    if (blank || draft.sending) return;
    const to = [...recipients];
    if (await mail.sendReply(draft.threadId)) toast.success(`Sent to ${joinNames(to)}.`);
  }
</script>

<form class="reply" aria-label="Reply to the thread" onsubmit={(event) => void send(event)}>
  <p class="reply-to">
    <Icon name="users" size={14} />
    <span>To: <strong>{joinNames(recipients)}</strong></span>
  </p>
  <TextField
    label="Reply"
    labelHidden
    multiline
    rows={4}
    placeholder="Reply to everyone in this thread…"
    bind:value={draft.body}
    onkeydown={(event) => sendOnModEnter(event, () => void send())}
  />
  <StagedFiles files={draft.files} />
  {#if draft.sendError !== null}
    <p class="reply-error" role="alert">{draft.sendError}</p>
  {/if}
  <div class="reply-foot">
    <AttachFiles files={draft.files} />
    <ComposerHint
      waiting={draft.sending && draft.files.uploading}
      delivery={deliveryHint(recipients)}
    />
    <Button type="submit" variant="primary" icon="send" disabled={blank} loading={draft.sending}>
      Send
    </Button>
  </div>
</form>

<style>
  /* A card of its own, like a message, so the reply reads as part of the thread. */
  .reply {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    padding: var(--space-12);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);
  }

  .reply-to {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);

    & strong {
      color: var(--color-text);
      font-weight: var(--font-weight-medium);
    }
  }

  .reply-error {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .reply-foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-10);
  }
</style>
