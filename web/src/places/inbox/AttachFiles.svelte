<script lang="ts">
  import type { StagedAttachments } from "../../lib/staged-attachments.svelte";
  import { Button } from "../../lib/ui";

  // "Attach files": a button over a hidden file picker. What is picked goes
  // to the message's files, which refuse any over the upload limit and start
  // uploading the rest at once.

  interface Props {
    files: StagedAttachments;
  }

  let { files }: Props = $props();

  let picker = $state<HTMLInputElement>();

  function picked(): void {
    const chosen = picker?.files;
    if (chosen !== null && chosen !== undefined && chosen.length > 0) files.attach(chosen);
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
