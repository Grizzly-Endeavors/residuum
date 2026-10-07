<script lang="ts">
  import { Icon } from "../lib/icons";
  import { instanceName } from "../lib/instance-slug";
  import { remoteAccess, type RemoteAccessStore } from "../lib/remote-access.svelte";
  import { Menu, MenuItem, VisuallyHidden } from "../lib/ui";

  // Which of the user's instances the shared address goes to, and a way to
  // move it. It shows only when Residuum Cloud lists more than one instance.
  // Names come from the relay and are drawn as text; an entry without a valid
  // slug never reaches `instances`, so every row can act.

  interface Props {
    store?: RemoteAccessStore;
  }

  let { store = remoteAccess }: Props = $props();

  const instances = $derived(store.instances);
  const active = $derived(instances.find((instance) => instance.active));
  const target = $derived(instances.find((instance) => instance.slug === store.switching));
  const current = $derived.by(() => {
    if (target !== undefined) return `Switching to ${instanceName(target)}…`;
    return active === undefined ? "Choose an instance" : instanceName(active);
  });

  function hintFor(instance: { active: boolean; connected: boolean }): string | undefined {
    if (instance.active) return instance.connected ? "Active" : "Active, offline";
    return instance.connected ? undefined : "Offline";
  }
</script>

{#if instances.length > 1}
  <div class="switcher">
    <Menu label="Switch instance" side="bottom" align="start">
      {#snippet trigger(props)}
        <button
          type="button"
          class="switcher-button"
          disabled={store.switching !== null}
          aria-label="Instance: {current}"
          {...props}
        >
          <Icon name="layers" size={15} />
          <span class="switcher-name">{current}</span>
          <Icon name="chevron-down" size={14} />
        </button>
      {/snippet}
      {#each instances as instance (instance.slug)}
        <MenuItem
          label={instanceName(instance)}
          hint={hintFor(instance)}
          onselect={() => void store.activate(instance.slug)}
        />
      {/each}
    </Menu>
    <VisuallyHidden>
      <span role="status">{store.switching === null ? "" : "Switching…"}</span>
    </VisuallyHidden>
  </div>
{/if}

<style>
  .switcher {
    flex: none;
    margin: 0 var(--space-10) var(--space-4);
  }

  .switcher-button {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    width: 100%;
    height: 34px;
    padding: 0 var(--space-8) 0 var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    text-align: start;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover:not(:disabled) {
      background: var(--color-stone-3);
      color: var(--color-text);
    }

    &:disabled {
      color: var(--color-text-3);
    }
  }

  .switcher-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 760px) {
    .switcher-button {
      height: var(--layout-touch-target);
    }
  }
</style>
