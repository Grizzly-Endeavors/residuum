<script lang="ts">
  import { configFileName, type ConfigConflict } from "../../lib/config-coordinator";
  import { Button, Dialog } from "../../lib/ui";
  import { conflictQuestion } from "./changed-on-disk.svelte";

  // Asks what to do when a file changed on disk under the keys a save is
  // about to write. Keeping the user's changes can be undone after the save;
  // using what's on disk drops their changes to that file. Closing the dialog
  // leaves the file unsaved, with its changes still staged.

  const conflict = $derived(conflictQuestion.current);

  function title({ file }: ConfigConflict): string {
    const name = file.kind === "hub" ? "The install-wide config.toml" : configFileName(file);
    return file.kind === "hub" ? `${name} changed` : `${file.agent}'s ${name} changed`;
  }

  function description({ keys }: ConfigConflict): string {
    const what = keys.length === 0 ? "This file" : keys.join(", ");
    return `${what} changed on disk after you started editing.`;
  }
</script>

<Dialog
  open={conflict !== null}
  title={conflict === null ? "" : title(conflict)}
  description={conflict === null ? undefined : description(conflict)}
  initialFocus="[data-autofocus]"
  onclose={() => {
    conflictQuestion.answer(null);
  }}
>
  <p class="conflict-note">
    Keep my changes saves yours over it. Use what's on disk drops your changes to this file.
  </p>
  {#snippet actions()}
    <Button variant="secondary" onclick={() => conflictQuestion.answer("use-disk")}>
      Use what's on disk
    </Button>
    <Button variant="primary" data-autofocus onclick={() => conflictQuestion.answer("keep-mine")}>
      Keep my changes
    </Button>
  {/snippet}
</Dialog>

<style>
  .conflict-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }
</style>
