<script lang="ts">
  import VisuallyHidden from "./VisuallyHidden.svelte";

  // A placeholder in the shape of content that is still loading. It pulses
  // gently so it doesn't read as empty.
  interface Props {
    shape?: "text" | "block" | "circle";
    /** Text only: how many lines; the last one is shorter. */
    lines?: number;
    /** A CSS length: the block's or the lines' width. */
    width?: string;
    /** A CSS length: the block's height, or the circle's size. */
    height?: string;
    /**
     * What is loading, announced once: "Loading sessions". Give it to one
     * skeleton per loading region and leave the rest silent.
     */
    label?: string;
  }

  let { shape = "text", lines = 1, width, height, label }: Props = $props();
</script>

<span class="ui-skeleton" data-shape={shape} role={label === undefined ? undefined : "status"}>
  {#if shape === "text"}
    {#each { length: lines }, index (index)}
      <span
        class="ui-skeleton-bone"
        aria-hidden="true"
        style:width={lines > 1 && index === lines - 1 ? "60%" : width}
      ></span>
    {/each}
  {:else}
    <span
      class="ui-skeleton-bone"
      aria-hidden="true"
      style:width={shape === "circle" ? height : width}
      style:height
    ></span>
  {/if}
  {#if label !== undefined}
    <VisuallyHidden>{label}</VisuallyHidden>
  {/if}
</span>

<style>
  .ui-skeleton {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    width: 100%;
  }

  .ui-skeleton-bone {
    display: block;
    height: 0.75em;
    border-radius: var(--corner-sm);
    background: var(--color-stone-3);
    animation: ui-skeleton-pulse var(--duration-pulse) var(--ease-in-out) infinite;
  }

  [data-shape="block"] > .ui-skeleton-bone {
    height: 48px;
    border-radius: var(--corner-md);
  }

  [data-shape="circle"] {
    width: auto;

    & > .ui-skeleton-bone {
      width: 32px;
      height: 32px;
      border-radius: 50%;
    }
  }

  @keyframes ui-skeleton-pulse {
    50% {
      opacity: 0.5;
    }
  }
</style>
