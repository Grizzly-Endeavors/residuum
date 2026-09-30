<script lang="ts">
  import type { Snippet } from "svelte";
  import VisuallyHidden from "./VisuallyHidden.svelte";
  import type { BadgeTone } from "./types";

  // A count ("3", "99+") or a short state label ("running", "overlap"). A
  // count of zero draws nothing.
  interface Props {
    count?: number;
    /** Counts above this read as "max+". */
    max?: number;
    /** What the count counts, for assistive technology: "unread" reads as "3 unread". */
    label?: string;
    tone?: BadgeTone;
    /** A filled accent count, for counts that need the user. Accent tone only. */
    solid?: boolean;
    /** A leading dot in the tone's color, for state labels. */
    dot?: boolean;
    children?: Snippet;
  }

  let { count, max = 99, label, tone, solid = false, dot = false, children }: Props = $props();

  const shown = $derived(count !== undefined && count > max ? `${max}+` : String(count));
</script>

{#if count !== undefined}
  {#if count > 0}
    <span
      class="ui-badge ui-badge-count"
      data-tone={tone ?? "accent"}
      data-solid={(solid && (tone ?? "accent") === "accent") || undefined}
    >
      {#if label}
        <span aria-hidden="true">{shown}</span>
        <VisuallyHidden>{`${count} ${label}`}</VisuallyHidden>
      {:else}
        {shown}
      {/if}
    </span>
  {/if}
{:else if children}
  <span class="ui-badge" data-tone={tone ?? "neutral"}>
    {#if dot}
      <span class="ui-badge-dot" aria-hidden="true"></span>
    {/if}
    {@render children()}
  </span>
{/if}

<style>
  .ui-badge {
    position: relative;
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: var(--space-6);
    height: 20px;
    padding: 0 var(--space-8);
    border-radius: var(--corner-pill);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
    line-height: 1;
    white-space: nowrap;
  }

  .ui-badge-count {
    justify-content: center;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    font-weight: var(--font-weight-semibold);
    font-variant-numeric: tabular-nums;
  }

  .ui-badge-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dot-color);
  }

  /* Text on a tint takes text, text-2 or vein-bright only; the dot carries the tone. */
  .ui-badge[data-tone="neutral"] {
    --dot-color: var(--color-text-3);

    background: var(--color-stone-3);
    color: var(--color-text-2);
  }

  .ui-badge[data-tone="accent"] {
    --dot-color: var(--color-vein-bright);

    background: var(--color-vein-tint);
    color: var(--color-vein-bright);
  }

  .ui-badge[data-tone="positive"] {
    --dot-color: var(--color-moss-text);

    background: var(--color-moss-tint);
    color: var(--color-text);
  }

  .ui-badge[data-tone="danger"] {
    --dot-color: var(--color-err-text);

    background: var(--color-err-tint);
    color: var(--color-text);
  }

  .ui-badge[data-solid] {
    background: var(--color-vein-bright);
    color: var(--color-stone-0);
  }
</style>
