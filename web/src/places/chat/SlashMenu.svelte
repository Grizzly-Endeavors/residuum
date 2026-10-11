<script lang="ts">
  import type { AppAction } from "../../lib/action-registry.svelte";
  import ActionOption from "../../shell/ActionOption.svelte";

  // The composer's `/` menu: the chat actions from the registry, above the
  // message box, which keeps focus and points at the active one. Option ids
  // are the menu's id and the index: `${id}-${index}`.

  interface Props {
    id: string;
    actions: readonly AppAction[];
    active: number;
    onpick: (action: AppAction) => void;
    onhover: (index: number) => void;
  }

  let { id, actions, active, onpick, onhover }: Props = $props();

  const optionId = (index: number): string => `${id}-${String(index)}`;

  $effect(() => {
    document.getElementById(optionId(active))?.scrollIntoView({ block: "nearest" });
  });
</script>

<div {id} class="slash-menu" role="listbox" aria-label="Chat actions">
  {#each actions as action, index (action.id)}
    <ActionOption
      {action}
      id={optionId(index)}
      active={index === active}
      hint={action.command === undefined ? undefined : `/${action.command}`}
      onpick={() => {
        onpick(action);
      }}
      onhover={() => {
        onhover(index);
      }}
    />
  {/each}
</div>

<style>
  .slash-menu {
    position: absolute;
    right: 0;
    bottom: calc(100% + var(--space-6));
    left: 0;
    z-index: var(--z-sticky);
    max-height: min(320px, 50vh);
    padding: var(--space-6);
    overflow-y: auto;
    border-radius: var(--corner-lg);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
  }
</style>
