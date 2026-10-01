<script lang="ts">
  import type { Snippet } from "svelte";

  // A card inside a settings section: an optional title with a state mark
  // beside it, a line on what the group is, then its fields. Groups sit on
  // stone-2 inside the modal's stone-1 and stack with a gap between.

  interface Props {
    title?: string;
    lede?: string;
    /** A state mark beside the title, such as a Badge. */
    status?: Snippet;
    children: Snippet;
  }

  let { title, lede, status, children }: Props = $props();

  const uid = $props.id();
  const titleId = `${uid}-title`;
</script>

<section class="set-group" aria-labelledby={title === undefined ? undefined : titleId}>
  {#if title !== undefined}
    <header class="set-group-head">
      <h3 id={titleId} class="set-group-title">{title}</h3>
      {@render status?.()}
    </header>
  {/if}
  {#if lede !== undefined}
    <p class="set-group-lede">{lede}</p>
  {/if}
  {@render children()}
</section>

<style>
  .set-group {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    max-width: 640px;
    margin-bottom: var(--space-16);
    padding: var(--space-18) var(--space-18) var(--space-20);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
  }

  .set-group-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-10);
  }

  .set-group-title {
    font-size: var(--font-size-message);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .set-group-lede {
    margin-top: calc(var(--space-4) * -1);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }
</style>
