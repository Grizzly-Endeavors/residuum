<script lang="ts">
  import { mail } from "../../lib/mail.svelte";
  import { Button } from "../../lib/ui";

  // The New message form. This minimal view keeps the Inbox place whole until
  // the full composer (recipients, subject, body, attachments) replaces it.

  interface Props {
    /** The recipients the form opens with. */
    to: string[];
    /** The message went: the new thread's id. */
    onsent: (threadId: string) => void;
    /** Closed without sending. */
    oncancel: () => void;
  }

  let { to, onsent, oncancel }: Props = $props();

  async function send(): Promise<void> {
    const threadId = await mail.sendNewMessage();
    if (threadId !== null) onsent(threadId);
  }
</script>

<form
  class="mail-compose"
  aria-label="New message"
  onsubmit={(event) => {
    event.preventDefault();
    void send();
  }}
>
  <h2>New message</h2>
  <p>To: {to.join(", ")}</p>
  <Button variant="quiet" onclick={oncancel}>Cancel</Button>
  <Button variant="primary" type="submit">Send</Button>
</form>
