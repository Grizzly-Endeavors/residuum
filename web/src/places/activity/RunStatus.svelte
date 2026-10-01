<script lang="ts">
  import type { RunStatus } from "../../lib/session-format";

  // How a run or task is doing, in a word or two after a dot in its tone: a
  // working one glows in the vein, a finished one is moss, a failed one red.

  let { status }: { status: RunStatus } = $props();
</script>

<span class="run-status" data-tone={status.tone}>
  <span class="run-status-dot" aria-hidden="true"></span>{status.text}
</span>

<style>
  .run-status {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;

    &[data-tone="working"] {
      color: var(--color-vein-bright);
    }

    &[data-tone="done"] {
      color: var(--color-moss-text);
    }

    &[data-tone="failed"] {
      color: var(--color-err-text);
    }
  }

  .run-status-dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentcolor;

    [data-tone="working"] > & {
      animation: run-status-pulse var(--duration-pulse) var(--ease-in-out) infinite;
    }
  }

  @keyframes run-status-pulse {
    50% {
      box-shadow: 0 0 0 4px var(--color-vein-faint);
    }
  }
</style>
