<script lang="ts">
  import type { StagedAttachment, StagedAttachments } from "../lib/staged-attachments.svelte";
  import { AttachmentChip, Button, type AttachmentState } from "../lib/ui";

  // The files staged on a message being written, each uploading, uploaded or
  // failed, and the note about files that were too large to attach. Every
  // chip can be removed before the message is sent; a failed one can be
  // uploaded again. The mail composers and the chat composer show the same.

  interface Props {
    files: StagedAttachments;
  }

  let { files }: Props = $props();

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

{#if files.notice !== null}
  <p class="staged-notice" role="status">
    <span>{files.notice}</span>
    <Button variant="quiet" size="sm" onclick={() => files.dismissNotice()}>Dismiss</Button>
  </p>
{/if}
{#if files.attachments.length > 0}
  <ul class="staged-files" aria-label="Attached files">
    {#each files.attachments as attachment (attachment.key)}
      <li>
        <AttachmentChip
          filename={attachment.filename}
          size={attachment.size}
          contentType={attachment.contentType || undefined}
          state={stateOf(attachment)}
          failure={attachment.error ?? undefined}
          onretry={() => void files.retry(attachment.key)}
          onremove={() => files.remove(attachment.key)}
        />
      </li>
    {/each}
  </ul>
{/if}

<style>
  .staged-notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-8);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .staged-files {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    list-style: none;
  }
</style>
