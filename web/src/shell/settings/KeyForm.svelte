<script lang="ts">
  import type { Snippet } from "svelte";
  import { Button } from "../../lib/ui";

  // The form that adds an entry to a saved list: its fields, then Cancel and
  // the button that adds it. It acts at once, so the button names the action
  // ("Save key", "Create key") instead of saying "Save changes".

  interface Props {
    /** Names the form for assistive technology. */
    label: string;
    /** The button that adds the entry. */
    submitLabel: string;
    /** The entry is being added: the button shows a spinner and Cancel is off. */
    saving: boolean;
    /** Whether the fields hold enough to add the entry. */
    ready: boolean;
    onsubmit: () => void;
    oncancel: () => void;
    children: Snippet;
  }

  let { label, submitLabel, saving, ready, onsubmit, oncancel, children }: Props = $props();
</script>

<form
  class="key-form"
  aria-label={label}
  onsubmit={(event) => {
    event.preventDefault();
    if (ready && !saving) onsubmit();
  }}
>
  {@render children()}
  <div class="key-form-actions">
    <Button variant="quiet" disabled={saving} onclick={oncancel}>Cancel</Button>
    <Button type="submit" variant="primary" loading={saving} disabled={!ready}>
      {submitLabel}
    </Button>
  </div>
</form>

<style>
  .key-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    padding-top: var(--space-16);
    border-top: 1px solid var(--color-line);
  }

  .key-form-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
  }
</style>
