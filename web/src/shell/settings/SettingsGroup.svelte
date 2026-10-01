<script lang="ts">
  import type { Snippet } from "svelte";

  // A card inside a settings section: a heading, a line on what it holds, and
  // its fields. Related settings sit in one card, set apart from the next by
  // surface tone, with no outline. A `foot` is a quiet action under the
  // fields, such as a link to the place that shows what the group configures.

  interface Props {
    title?: string;
    lede?: string;
    foot?: Snippet;
    children: Snippet;
  }

  let { title, lede, foot, children }: Props = $props();

  const uid = $props.id();
  const titleId = `${uid}-title`;
</script>

<div class="settings-group" role="group" aria-labelledby={title ? titleId : undefined}>
  {#if title}
    <div class="settings-group-head">
      <h3 id={titleId} class="settings-group-title">{title}</h3>
      {#if lede}
        <p class="settings-group-lede">{lede}</p>
      {/if}
    </div>
  {/if}
  {@render children()}
  {#if foot}
    <div class="settings-group-foot">{@render foot()}</div>
  {/if}
</div>

<style>
  .settings-group {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    max-width: 640px;
    margin-bottom: var(--space-16);
    padding: var(--space-18) var(--space-18) var(--space-20);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
  }

  .settings-group-head {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .settings-group-title {
    font-size: var(--font-size-message);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .settings-group-foot {
    display: flex;
    margin: calc(var(--space-4) * -1) 0 calc(var(--space-4) * -1) calc(var(--space-8) * -1);
  }

  .settings-group-lede {
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    color: var(--color-text-2);
  }
</style>
