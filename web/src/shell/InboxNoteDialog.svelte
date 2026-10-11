<script lang="ts">
  import { Button, Dialog, TextField } from "../lib/ui";

  // Asks for the text of a note for an agent's inbox, for "Add a note to
  // <agent>'s inbox" run without text.

  interface Props {
    /** The agent the note is for, or null while closed. */
    agent: string | null;
    onadd: (text: string) => void;
    onclose: () => void;
  }

  let { agent, onadd, onclose }: Props = $props();

  let text = $state("");

  function add(event: SubmitEvent): void {
    event.preventDefault();
    const body = text.trim();
    if (body === "") return;
    text = "";
    onadd(body);
  }
</script>

<Dialog
  open={agent !== null}
  title="Add a note to {agent ?? ''}'s inbox"
  description="It goes in the queue {agent ?? 'the agent'} works through itself, not in your Inbox."
  size="md"
  {onclose}
>
  <form id="inbox-note" onsubmit={add}>
    <TextField label="Note" multiline rows={4} bind:value={text} data-autofocus />
  </form>
  {#snippet actions()}
    <Button variant="quiet" onclick={onclose}>Cancel</Button>
    <Button variant="primary" type="submit" form="inbox-note" disabled={text.trim() === ""}>
      Add note
    </Button>
  {/snippet}
</Dialog>
