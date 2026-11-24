<script lang="ts">
  import type { Snippet } from "svelte";
  import { Button } from "../../lib/ui";

  interface Props {
    label: string;
    submitLabel: string;
    saving: boolean;
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
