<script lang="ts">
  // Text the agent's connection sent for this page only (`inline_output`). It
  // isn't part of the conversation and is gone after a reload.

  let { content }: { content: string } = $props();
</script>

<pre class="local-note">{content}</pre>

<style>
  .local-note {
    padding: var(--space-10) var(--space-12);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
    color: var(--color-text-2);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
