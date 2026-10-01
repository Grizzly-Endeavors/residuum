<script lang="ts">
  import { Icon, type IconName } from "../lib/icons";
  import { userInbox } from "../lib/inbox.svelte";
  import { router } from "../lib/router.svelte";
  import { formatLocation, HOME, locationAt, type Place } from "../lib/routes";
  import { Badge } from "../lib/ui";
  import type { ShellActions } from "./shell-actions";

  // The phone's bottom bar, on every place: the menu that opens the rail as a
  // drawer, Inbox, Home in the middle, and Settings. The drawer and every
  // modal layer cover it.

  interface Props {
    drawerOpen: boolean;
    onmenu: () => void;
    actions: ShellActions;
  }

  let { drawerOpen, onmenu, actions }: Props = $props();

  const INBOX: Place = { kind: "inbox", agent: null, tab: "active", item: null };

  function open(event: MouseEvent, target: Place): void {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) {
      return;
    }
    event.preventDefault();
    void router.openPlace(target);
  }
</script>

{#snippet placeTab(target: Place, label: string, icon: IconName, count = 0)}
  <a
    class="bar-tab"
    href={formatLocation(locationAt(target))}
    aria-current={router.place.kind === target.kind && router.settings === null
      ? "page"
      : undefined}
    onclick={(event) => open(event, target)}
  >
    <span class="bar-icon"><Icon name={icon} size={21} /></span>
    {label}
    <span class="bar-count"><Badge {count} label="unread" solid /></span>
  </a>
{/snippet}

<nav class="shell-bar" aria-label="Main">
  <button
    type="button"
    class="bar-tab"
    aria-haspopup="dialog"
    aria-expanded={drawerOpen}
    onclick={onmenu}
  >
    <span class="bar-icon"><Icon name="menu" size={21} /></span>
    Menu
  </button>
  {@render placeTab(INBOX, "Inbox", "inbox", userInbox.unreadCount)}
  {@render placeTab(HOME, "Home", "home")}
  <button
    type="button"
    class="bar-tab"
    aria-current={router.settings === null ? undefined : "page"}
    onclick={actions.openSettings}
  >
    <span class="bar-icon"><Icon name="settings" size={21} /></span>
    Settings
  </button>
</nav>

<style>
  .shell-bar {
    position: fixed;
    right: 0;
    bottom: 0;
    left: 0;
    z-index: var(--z-sticky);
    display: none;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    height: var(--layout-bottom-bar-offset);
    padding-bottom: env(safe-area-inset-bottom, 0px);
    border-top: 1px solid var(--color-line-soft);
    background: var(--color-stone-1);
  }

  @media (max-width: 760px) {
    .shell-bar {
      display: grid;
    }
  }

  .bar-tab {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    min-width: 0;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    text-decoration: none;
    transition: color var(--duration-fast) var(--ease-out);

    &[aria-current="page"],
    &[aria-expanded="true"] {
      color: var(--color-text);

      & .bar-icon {
        color: var(--color-vein-bright);
      }
    }
  }

  .bar-icon {
    display: grid;
  }

  /* The count sits on the icon's top corner. */
  .bar-count {
    position: absolute;
    top: var(--space-4);
    left: calc(50% + var(--space-4));
  }
</style>
