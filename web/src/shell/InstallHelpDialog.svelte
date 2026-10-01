<script lang="ts">
  import { Button, Dialog } from "../lib/ui";

  // Install app on an iPhone or iPad, which has no install prompt: the person
  // adds Residuum to the Home Screen from the browser's share sheet.

  let { open = $bindable(false) }: { open?: boolean } = $props();
</script>

<Dialog
  bind:open
  title="Add Residuum to your Home Screen"
  description="It opens full screen, like an app, and shows your unread count on its icon."
  size="sm"
>
  <ol class="install-steps">
    <li>Tap the Share button in your browser's toolbar.</li>
    <li>Scroll down and tap <strong>Add to Home Screen</strong>.</li>
    <li>Tap <strong>Add</strong>, then open Residuum from your Home Screen.</li>
  </ol>
  {#snippet actions()}
    <Button variant="primary" onclick={() => (open = false)}>Done</Button>
  {/snippet}
</Dialog>

<style>
  .install-steps {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    padding-left: var(--space-20);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);

    & strong {
      color: var(--color-text);
      font-weight: var(--font-weight-medium);
    }
  }
</style>
