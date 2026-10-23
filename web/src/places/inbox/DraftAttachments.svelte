<script lang="ts">
  import type { MailDraft, StagedAttachment } from "../../lib/mail.svelte";
  import { AttachmentChip, Button, type AttachmentState } from "../../lib/ui";

  // The files staged on a draft, each uploading, uploaded or failed, and the
  // note about files that were too large to attach. Every chip can be removed
  // before the message is sent; a failed one can be uploaded again.

  interface Props {
    draft: MailDraft;
  }

  let { draft }: Props = $props();

  function stateOf(attachment: StagedAttachment): AttachmentState {
    switch (attachment.status) {
      case "uploading":
        return "pending";
      case "failed":
        return "failed";
      case "uploaded":
        return "ready";
    }
  }
</script>

{#if draft.attachNotice !== null}
  <p class="draft-notice" role="status">
    <span>{draft.attachNotice}</span>
    <Button variant="quiet" size="sm" onclick={() => draft.dismissAttachNotice()}>Dismiss</Button>
  </p>
{/if}
{#if draft.attachments.length > 0}
  <ul class="draft-files" aria-label="Attached files">
    {#each draft.attachments as attachment (attachment.key)}
      <li>
        <AttachmentChip
          filename={attachment.filename}
          size={attachment.size}
          contentType={attachment.file.type || undefined}
          state={stateOf(attachment)}
          failure={attachment.error ?? undefined}
          onretry={() => void draft.retryAttachment(attachment.key)}
          onremove={() => draft.removeAttachment(attachment.key)}
        />
      </li>
    {/each}
  </ul>
{/if}

<style>
  .draft-notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-8);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .draft-files {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    list-style: none;
  }
</style>
