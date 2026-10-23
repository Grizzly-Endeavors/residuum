<script lang="ts">
  import type { MailDraft } from "../../lib/mail.svelte";
  import { Button } from "../../lib/ui";

  // "Attach files": a button over a hidden file picker. What is picked goes
  // to the draft, which refuses files over the upload limit and starts
  // uploading the rest at once.

  interface Props {
    draft: MailDraft;
  }

  let { draft }: Props = $props();

  let picker = $state<HTMLInputElement>();

  function picked(): void {
    const files = picker?.files;
    if (files !== null && files !== undefined && files.length > 0) draft.attach(files);
    // Cleared so picking the same file again (after removing it) still reports a change.
    if (picker !== undefined) picker.value = "";
  }
</script>

<Button variant="quiet" size="sm" icon="paperclip" onclick={() => picker?.click()}>
  Attach files
</Button>
<input
  bind:this={picker}
  class="attach-picker"
  type="file"
  multiple
  tabindex="-1"
  aria-label="Files to attach"
  onchange={picked}
/>

<style>
  .attach-picker {
    display: none;
  }
</style>
