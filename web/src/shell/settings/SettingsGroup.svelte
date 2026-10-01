<script lang="ts">
  import type { Snippet } from "svelte";

  // A titled group of related fields inside a settings section: a well on the
  // modal's surface, its heading, a line on what it does, then the fields. A
  // state mark goes beside the title (`status`), and actions that belong to
  // the whole group sit at the end of that row.

  interface Props {
    title: string;
    /** What the group does, in a sentence. */
    lede?: string;
    status?: Snippet;
    actions?: Snippet;
    children: Snippet;
  }

  let { title, lede, status, actions, children }: Props = $props();

  const uid = $props.id();
</script>

<section class="group-card" aria-labelledby="{uid}-title">
  <div class="group-head">
    <h3 id="{uid}-title" class="group-title">{title}</h3>
    {@render status?.()}
    {#if actions}
      <div class="group-actions">{@render actions()}</div>
    {/if}
  </div>
  {#if lede}
    <p class="group-lede">{lede}</p>
  {/if}
  {@render children()}
</section>

<style>
  .group-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    max-width: 640px;
    margin-bottom: var(--space-16);
    padding: var(--space-18) var(--space-18) var(--space-20);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
  }

  .group-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8) var(--space-10);
  }

  .group-title {
    font-size: var(--font-size-message);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .group-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
    margin-left: auto;
  }

  .group-lede {
    margin-top: calc(var(--space-4) * -1);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    color: var(--color-text-2);
  }

  @media (max-width: 760px) {
    .group-card {
      padding: var(--space-14) var(--space-14) var(--space-16);
    }
  }
</style>
