<script lang="ts">
  import type { Snippet } from "svelte";

  // Related fields on a setup step, grouped by surface tone rather than a
  // border. A titled group is a named region under the step's heading.
  interface Props {
    title?: string;
    /** A line under the title: plain text, or markup with links. */
    hint?: string | Snippet;
    children: Snippet;
  }

  let { title, hint, children }: Props = $props();

  const uid = $props.id();
</script>

<section class="setup-group" aria-labelledby={title === undefined ? undefined : `${uid}-title`}>
  {#if title !== undefined || hint !== undefined}
    <div class="setup-group-head">
      {#if title !== undefined}
        <h2 id="{uid}-title" class="setup-group-title">{title}</h2>
      {/if}
      {#if typeof hint === "string"}
        <p class="setup-group-hint">{hint}</p>
      {:else if hint}
        <p class="setup-group-hint">{@render hint()}</p>
      {/if}
    </div>
  {/if}
  {@render children()}
</section>

<style>
  .setup-group {
    container: setup-group / inline-size;
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    padding: var(--space-18) var(--space-18) var(--space-20);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);
  }

  .setup-group-head {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .setup-group-title {
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .setup-group-hint {
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    color: var(--color-text-2);
  }

  @media (max-width: 760px) {
    .setup-group {
      padding: var(--space-16);
    }
  }
</style>
