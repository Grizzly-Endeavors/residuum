<script lang="ts">
  import { actionRegistry, type AppAction } from "../../lib/action-registry.svelte";
  import { hub } from "../../lib/hub.svelte";
  import type { IconName } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import { IconButton, Menu, MenuItem, VisuallyHidden } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";

  // The Chat's title bar: the agent with its role, how many of its sessions
  // are running (opening Activity), its settings, and a menu with the
  // conversation's size, Restart and Stop. The menu's items are the action
  // registry's, so they carry the same reasons when they can't run.

  let { agent }: { agent: string } = $props();

  const summary = $derived(hub.agent(agent));
  const running = $derived(ws.agent === agent ? ws.sessions.live.length : 0);

  /** Why Restart or Stop isn't offered, when the registry doesn't list it. */
  const lifecycleReason = $derived.by(() => {
    if (hub.isStopping(agent)) return `${agent} is stopping`;
    if (summary?.state === "starting") return `${agent} is still starting`;
    return `${agent} isn't running`;
  });

  interface MenuEntry {
    label: string;
    icon: IconName;
    action: AppAction | undefined;
  }

  const entries = $derived.by((): MenuEntry[] => {
    const byId = (id: string): AppAction | undefined =>
      actionRegistry.all.find((action) => action.id === id);
    return [
      { label: "Show conversation size", icon: "memory", action: byId("chat:context") },
      { label: `Restart ${agent}`, icon: "reload", action: byId(`lifecycle:${agent}:restart`) },
      { label: `Stop ${agent}`, icon: "stop", action: byId(`lifecycle:${agent}:stop`) },
    ];
  });
</script>

<PlaceHeader title={agent} {agent} sub={summary?.role}>
  <div class="chat-header-actions">
    {#if running > 0}
      <button
        type="button"
        class="running-pill"
        onclick={() => void router.openPlace({ kind: "activity", agent })}
      >
        <span class="running-dot" aria-hidden="true"></span>
        {running} running<VisuallyHidden>, open Activity</VisuallyHidden>
      </button>
    {/if}
    <span class="chat-header-gear">
      <IconButton
        icon="settings"
        label="{agent} settings"
        onclick={() => void router.openSettings({ scope: agent, section: null })}
      />
    </span>
    <Menu label="More for {agent}" align="end">
      {#snippet trigger(props)}
        <IconButton icon="more" label="More for {agent}" {...props} />
      {/snippet}
      {#each entries as entry (entry.label)}
        {@const action = entry.action}
        <MenuItem
          label={entry.label}
          icon={entry.icon}
          disabled={action === undefined || action.disabled !== undefined}
          hint={action === undefined ? lifecycleReason : action.disabled}
          onselect={() => {
            if (action !== undefined) void actionRegistry.run(action);
          }}
        />
      {/each}
    </Menu>
  </div>
</PlaceHeader>

<style>
  .chat-header-actions {
    display: flex;
    flex: none;
    align-self: center;
    align-items: center;
    gap: var(--space-4);
    margin-left: auto;
  }

  .running-pill {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    min-height: 28px;
    margin-right: var(--space-4);
    padding: 0 var(--space-12);
    border-radius: var(--corner-pill);
    background: var(--color-vein-faint);
    color: var(--color-vein-bright);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
    white-space: nowrap;
    transition: background var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-vein-tint);
    }
  }

  .running-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentcolor;
  }

  @media (max-width: 760px) {
    .running-pill {
      min-height: var(--layout-touch-target);
    }

    /* Settings is in the bottom bar on phones. */
    .chat-header-gear {
      display: none;
    }
  }
</style>
