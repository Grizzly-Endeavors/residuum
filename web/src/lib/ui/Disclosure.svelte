<script lang="ts">
  import type { Snippet } from "svelte";
  import { Icon } from "../icons";

  // A button that shows and hides the content under it. Closed content stays
  // mounted but hidden, so fields inside keep what was typed.
  interface Props {
    summary: string;
    open?: boolean;
    /** `quiet` for secondary detail such as an error's full text. */
    tone?: "default" | "quiet";
    ontoggle?: (open: boolean) => void;
    children: Snippet;
  }

  let { summary, open = $bindable(false), tone = "default", ontoggle, children }: Props = $props();

  const uid = $props.id();
  const panelId = `${uid}-panel`;
</script>

<div class="ui-disclosure" data-tone={tone}>
  <button
    type="button"
    class="ui-disclosure-trigger"
    aria-expanded={open}
    aria-controls={panelId}
    onclick={() => {
      open = !open;
      ontoggle?.(open);
    }}
  >
    <span class="ui-disclosure-chevron"><Icon name="chevron-right" size={15} /></span>
    {summary}
  </button>
  <div id={panelId} class="ui-disclosure-panel" hidden={!open}>
    {@render children()}
  </div>
</div>

<style>
  .ui-disclosure {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    min-width: 0;
  }

  .ui-disclosure-trigger {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    min-height: 36px;
    padding: 0 var(--space-8) 0 var(--space-4);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    transition: color var(--duration-fast) var(--ease-out);

    &:hover {
      color: var(--color-text);
    }
  }

  [data-tone="quiet"] > .ui-disclosure-trigger {
    min-height: 28px;
    color: var(--color-text-3);
    font-weight: var(--font-weight-regular);

    &:hover {
      color: var(--color-text-2);
    }
  }

  .ui-disclosure-chevron {
    display: grid;
    transition: transform var(--duration-base) var(--ease-out);
  }

  [aria-expanded="true"] > .ui-disclosure-chevron {
    transform: rotate(90deg);
  }

  .ui-disclosure-panel {
    align-self: stretch;
    padding-top: var(--space-8);
  }

  @media (max-width: 760px) {
    .ui-disclosure-trigger,
    [data-tone="quiet"] > .ui-disclosure-trigger {
      min-height: var(--layout-touch-target);
    }
  }
</style>
