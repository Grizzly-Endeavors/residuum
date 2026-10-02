<script lang="ts">
  import { VisuallyHidden } from "../../lib/ui";

  // Where the user is in setup: one segment of vein per step, filled up to
  // the current one. Narrow, the step names give way to "Step 2 of 6"; the
  // list still names every step for assistive technology.
  interface Props {
    labels: readonly string[];
    current: number;
  }

  let { labels, current }: Props = $props();

  function stepState(index: number): "done" | "current" | "next" {
    if (index < current) return "done";
    return index === current ? "current" : "next";
  }
</script>

<div class="setup-progress">
  <ol class="setup-progress-steps" aria-label="Setup steps">
    {#each labels as label, index (label)}
      <li
        class="setup-progress-step"
        data-state={stepState(index)}
        aria-current={index === current ? "step" : undefined}
      >
        <span class="setup-progress-vein" aria-hidden="true"></span>
        <span class="setup-progress-label">
          {label}{#if index < current}<VisuallyHidden>, done</VisuallyHidden>{/if}
        </span>
      </li>
    {/each}
  </ol>
  <p class="setup-progress-count" aria-hidden="true">Step {current + 1} of {labels.length}</p>
</div>

<style>
  .setup-progress {
    container-type: inline-size;
  }

  .setup-progress-steps {
    display: grid;
    grid-auto-columns: minmax(0, 1fr);
    grid-auto-flow: column;
    gap: var(--space-4);
    list-style: none;
  }

  .setup-progress-step {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    min-width: 0;
  }

  .setup-progress-vein {
    height: 2px;
    border-radius: var(--corner-pill);
    background: var(--color-line);
    transition: background-color var(--duration-base) var(--ease-out);
  }

  .setup-progress-label {
    overflow: hidden;
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  [data-state="done"] {
    & > .setup-progress-vein {
      background: var(--color-vein);
    }

    & > .setup-progress-label {
      color: var(--color-text-2);
    }
  }

  [data-state="current"] {
    & > .setup-progress-vein {
      background: var(--color-vein-bright);
    }

    & > .setup-progress-label {
      font-weight: var(--font-weight-medium);
      color: var(--color-text);
    }
  }

  .setup-progress-count {
    display: none;
    margin-top: var(--space-8);
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
  }

  @container (max-width: 520px) {
    .setup-progress-label {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }

    .setup-progress-count {
      display: block;
    }
  }
</style>
