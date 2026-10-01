<script lang="ts">
  import type { AppAction } from "../lib/action-registry.svelte";
  import { Icon } from "../lib/icons";
  import { StatusDot } from "../lib/ui";

  // One action in a list the keyboard drives from a text field: the palette
  // and the composer's `/` menu. Focus stays in the field, which points at
  // the active option, so pointer and keyboard share one highlight. A
  // disabled action can be reached, its reason in place of its hint, and does
  // nothing.

  interface Props {
    action: AppAction;
    id: string;
    active: boolean;
    /** The quiet text at the end of the row. A disabled action shows its reason instead. */
    hint?: string;
    onpick: () => void;
    onhover: () => void;
  }

  let { action, id, active, hint, onpick, onhover }: Props = $props();

  const shownHint = $derived(action.disabled ?? hint);
</script>

<button
  type="button"
  {id}
  class="action-option"
  role="option"
  tabindex="-1"
  aria-selected={active}
  aria-disabled={action.disabled === undefined ? undefined : true}
  onpointerdown={(event) => {
    // Keep focus in the field that drives the list.
    event.preventDefault();
  }}
  onpointermove={() => {
    if (!active) onhover();
  }}
  onclick={onpick}
>
  <span class="action-icon">
    {#if typeof action.icon === "string"}
      <Icon name={action.icon} size={15} />
    {:else}
      <StatusDot state={action.icon.dot} working={action.icon.working} />
    {/if}
  </span>
  <span class="action-label">{action.label}</span>
  {#if shownHint !== undefined}
    <span class="action-hint">{shownHint}</span>
  {/if}
</button>

<style>
  /* Set out in full: the composer's menu sits where the base styles don't reach. */
  .action-option {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-10);
    width: 100%;
    min-height: 36px;
    margin: 0;
    padding: 0 var(--space-10);
    border: 0;
    border-radius: var(--corner-md);
    background: none;
    color: var(--color-text-2);
    font-family: var(--font-ui);
    font-size: var(--font-size-sm);
    text-align: start;
    cursor: pointer;

    &[aria-disabled="true"] {
      cursor: default;

      & .action-label {
        color: var(--color-text-3);
      }
    }

    /* The active row lights its icon and carries a short vein at its edge, as the rail's current place does. */
    &[aria-selected="true"] {
      background: var(--color-vein-tint);
      color: var(--color-text);

      &::before {
        content: "";
        position: absolute;
        top: var(--space-10);
        bottom: var(--space-10);
        left: 0;
        width: var(--space-2);
        border-radius: var(--corner-pill);
        background: var(--color-vein-bright);
      }

      & .action-icon {
        color: var(--color-vein-bright);
      }

      & .action-label,
      & .action-hint {
        color: var(--color-text-2);
      }

      &:not([aria-disabled="true"]) .action-label {
        color: var(--color-text);
      }
    }
  }

  .action-icon {
    display: grid;
    flex: none;
    place-items: center;
    width: 16px;
    color: var(--color-text-3);
  }

  .action-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .action-hint {
    flex: none;
    max-width: 50%;
    margin-left: auto;
    padding-left: var(--space-10);
    overflow: hidden;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 760px) {
    .action-option {
      min-height: 46px;
    }
  }
</style>
