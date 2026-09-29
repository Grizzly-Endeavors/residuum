<script lang="ts">
  import type { AgentState } from "../lib/hub-types";

  // Each state has its own shape as well as its own color, so the state reads
  // without relying on color. Callers supply the words (a label or aria-label).
  let { state }: { state: AgentState } = $props();
</script>

<svg
  class="agent-glyph state-{state}"
  viewBox="0 0 12 12"
  width="12"
  height="12"
  aria-hidden="true"
>
  {#if state === "running"}
    <circle cx="6" cy="6" r="4" fill="currentColor" />
  {:else if state === "starting"}
    <circle
      class="glyph-spin"
      cx="6"
      cy="6"
      r="4"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-dasharray="3 2.2"
    />
  {:else if state === "stopped"}
    <circle cx="6" cy="6" r="3.6" fill="none" stroke="currentColor" stroke-width="1.4" />
  {:else}
    <path d="M6 1.2 11 10.4H1z" fill="currentColor" />
    <path d="M6 4.6v2.7M6 8.4v.6" stroke="var(--bg-deep)" stroke-width="1.1" />
  {/if}
</svg>

<style>
  .agent-glyph {
    flex: none;
  }

  .state-running {
    color: var(--moss-hover);
  }
  .state-starting {
    color: var(--vein-bright);
  }
  .state-stopped {
    color: var(--text-dim);
  }
  .state-failed {
    color: var(--error);
  }

  .glyph-spin {
    transform-origin: center;
    animation: glyph-spin 2.4s linear infinite;
  }

  @keyframes glyph-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .glyph-spin {
      animation: none;
    }
  }
</style>
