<script lang="ts">
  import { displaySubject } from "../../lib/mail-subject";
  import { mail } from "../../lib/mail.svelte";
  import { Button } from "../../lib/ui";

  // The open thread. This minimal view keeps the Inbox place whole until the
  // full thread view (messages, reply composer, attachments) replaces it.

  interface Props {
    /** The open thread's id, or the ref of one of its messages until the address is canonical. */
    threadId: string;
    /** Back to the list. */
    onback: () => void;
  }

  let { threadId, onback }: Props = $props();
</script>

<article class="mail-thread" aria-label="Conversation" data-thread={threadId}>
  <Button variant="quiet" size="sm" icon="back" onclick={onback}>Back to list</Button>
  <h2>{mail.thread === null ? "Loading" : displaySubject(mail.thread.subject)}</h2>
</article>
