<script lang="ts">
  import { hub } from "../lib/hub.svelte";
  import { Icon } from "../lib/icons";

  // Where the summarized past meets the messages kept word for word.

  let { agent }: { agent: string } = $props();
</script>

<p class="compressed" role="note">
  <Icon name="layers" size={15} />
  <span>
    Older messages are summarized. {hub.shownName(agent)} remembers what was said, not the exact wording.
  </span>
</p>

<style>
  .compressed {
    display: flex;
    align-items: flex-start;
    gap: var(--space-10);
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);

    & :global(svg) {
      flex: none;
      margin-top: var(--space-2);
      color: var(--color-text-3);
    }
  }
</style>
