<script lang="ts">
  import { onMount, tick } from "svelte";
  import { actionRegistry } from "../lib/action-registry.svelte";
  import { showAppBadge } from "../lib/app-badge";
  import { hub } from "../lib/hub.svelte";
  import { LazyComponent } from "../lib/lazy-component.svelte";
  import { notifications } from "../lib/notifications.svelte";
  import { overview } from "../lib/overview.svelte";
  import { router } from "../lib/router.svelte";
  import { ws } from "../lib/ws.svelte";
  import { ALL_SCOPE } from "../lib/settings-sections";
  import {
    ConfirmHost,
    confirmLeave,
    Drawer,
    RecentNotifications,
    Spinner,
    VisuallyHidden,
  } from "../lib/ui";
  import { PHONE_QUERY } from "../styles/breakpoints";
  import { RailAccordion } from "./accordion.svelte";
  import { connectionStatusDialog, installHelp, registerAppActions } from "./app-actions.svelte";
  import BottomBar from "./BottomBar.svelte";
  import ConnectionStatusDialog from "./ConnectionStatusDialog.svelte";
  import CreateAgentDialog from "./CreateAgentDialog.svelte";
  import FeedbackDialog from "./FeedbackDialog.svelte";
  import HubBanner from "./HubBanner.svelte";
  import InboxNoteDialog from "./InboxNoteDialog.svelte";
  import InstallHelpDialog from "./InstallHelpDialog.svelte";
  import PanelHost from "./panel/PanelHost.svelte";
  import PlaceHost from "./PlaceHost.svelte";
  import Rail from "./Rail.svelte";
  import type { FeedbackTab, ShellActions } from "./shell-actions";
  import ShortcutsDialog from "./ShortcutsDialog.svelte";
  import UpdateBanner from "./UpdateBanner.svelte";

  // The frame around every place: the rail beside the main region at medium
  // and wide widths, and the context panel beside it (wide) or over it
  // (medium); on phones the bottom bar, with the rail in a drawer and the
  // panel a full-screen sheet. The shell also owns the overlays its controls
  // and the action registry open. Settings and the palette are built apart
  // from the shell and load the first time they open.

  router.guard.setConfirm(confirmLeave);

  /** The drawer opens on the row of the place the user is on. */
  const CURRENT_ROW = '[aria-current="page"]';

  const settingsModal = new LazyComponent(() => import("./SettingsModal.svelte"), "Settings");
  const commandPalette = new LazyComponent<{ open: boolean }>(
    () => import("./CommandPalette.svelte"),
    "search",
  );

  const accordion = new RailAccordion();
  let drawerOpen = $state(false);
  let notificationsOpen = $state(false);
  let shortcutsOpen = $state(false);
  let feedbackOpen = $state(false);
  let feedbackTab = $state<FeedbackTab>("bug");
  let paletteOpen = $state(false);
  let inboxNoteAgent = $state<string | null>(null);
  let createOpen = $state(false);
  let sideRail = $state<HTMLElement>();

  // A settings URL, from a link or a reload, opens the modal. If its code can't load, the modal is put away so Settings can be asked for again.
  $effect(() => {
    if (router.settings !== null) settingsModal.ensure(() => void router.closeSettings());
  });

  $effect(() => {
    if (paletteOpen) {
      commandPalette.ensure(() => {
        paletteOpen = false;
      });
    }
  });

  // The installed app's icon shows the inbox unread total, once it is known.
  $effect(() => {
    if (overview.loaded) showAppBadge(overview.inboxUnread);
  });

  // Arriving on an agent opens its places in the rail.
  $effect(() => {
    accordion.follow(router.viewedAgent);
  });

  const actions: ShellActions = {
    openSearch: () => {
      drawerOpen = false;
      paletteOpen = true;
    },
    openSettings: () => {
      drawerOpen = false;
      // The phone's bar stays in reach beside the open modal; pressing Settings there keeps it as it is.
      if (router.settings !== null) return;
      void router.openSettings({ scope: router.viewedAgent ?? ALL_SCOPE, section: null });
    },
    openShortcuts: () => {
      shortcutsOpen = true;
    },
    openNotifications: () => {
      notificationsOpen = true;
    },
    openFeedback: (tab) => {
      feedbackTab = tab;
      feedbackOpen = true;
    },
    // The user stays where they are: the dialog opens over the current place.
    createAgent: () => {
      drawerOpen = false;
      createOpen = true;
    },
    addInboxNote: (agent) => {
      inboxNoteAgent = agent;
    },
  };

  /** A new agent's rail row takes focus, where the rail shows beside the main region (not on phones). */
  async function focusCreatedAgent(name: string): Promise<void> {
    await tick();
    sideRail?.querySelector<HTMLElement>(`[data-rail-agent="${CSS.escape(name)}"]`)?.focus();
  }

  function addInboxNote(text: string): void {
    inboxNoteAgent = null;
    const action = actionRegistry.all.find((candidate) => candidate.id === "chat:inbox");
    if (action?.disabled !== undefined) {
      notifications.surface("error", `Couldn't add the note: ${action.disabled}.`);
    } else if (action !== undefined) {
      void actionRegistry.run(action, text);
    }
  }

  // Whatever an action opens or wherever it goes, the drawer it may have been
  // run from gets out of the way first.
  onMount(() => {
    const removeActions = registerAppActions(actions);
    const stopListening = actionRegistry.onRun(() => {
      drawerOpen = false;
    });
    return () => {
      removeActions();
      stopListening();
    };
  });

  onMount(() => {
    // The drawer is the phone's rail; a wider window shows the rail itself.
    const phone = window.matchMedia(PHONE_QUERY);
    const leftPhoneWidth = (): void => {
      if (!phone.matches) drawerOpen = false;
    };
    phone.addEventListener("change", leftPhoneWidth);
    return () => {
      phone.removeEventListener("change", leftPhoneWidth);
    };
  });

  function handleKeydown(event: KeyboardEvent): void {
    if (event.defaultPrevented) return;
    // ⌘K or Ctrl+K opens the palette from anywhere, and closes it again.
    const mod = event.metaKey || event.ctrlKey;
    if (mod && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (paletteOpen) paletteOpen = false;
      else actions.openSearch();
      return;
    }
    // `?` opens the shortcuts, unless something is taking text.
    if (event.key !== "?") return;
    const target = event.target as HTMLElement | null;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target?.isContentEditable) {
      return;
    }
    event.preventDefault();
    drawerOpen = false;
    shortcutsOpen = true;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- data-hub and data-overview say how far the hub's state has arrived (the socket, then the counts it brings), for the end-to-end suite, which waits for "connected" and "loaded" before it acts. -->
<div
  class="shell"
  data-hub={hub.transport.lost ? "lost" : hub.transport.status}
  data-overview={overview.loaded ? "loaded" : "loading"}
>
  <div class="shell-side" bind:this={sideRail}>
    <Rail {accordion} {actions} />
  </div>
  <main class="shell-main">
    <UpdateBanner />
    <HubBanner />
    <PlaceHost {actions} />
  </main>
  <PanelHost />
  <BottomBar {drawerOpen} onmenu={() => (drawerOpen = !drawerOpen)} {actions} />
</div>

<Drawer bind:open={drawerOpen} label="Agents and places" initialFocus={CURRENT_ROW}>
  <Rail {accordion} {actions} onclose={() => (drawerOpen = false)} />
</Drawer>
{#if settingsModal.loading || commandPalette.loading}
  <!-- Fades in after a moment, so a chunk that arrives quickly never flashes it. -->
  <div class="shell-opening" role="status">
    <Spinner size={20} />
    <VisuallyHidden>Opening</VisuallyHidden>
  </div>
{/if}
{#if settingsModal.component !== null}
  {@const Settings = settingsModal.component}
  <Settings />
{/if}
{#if commandPalette.component !== null}
  {@const Palette = commandPalette.component}
  <Palette bind:open={paletteOpen} />
{/if}
<CreateAgentDialog bind:open={createOpen} oncreated={(name) => void focusCreatedAgent(name)} />
<RecentNotifications bind:open={notificationsOpen} />
<ShortcutsDialog bind:open={shortcutsOpen} />
<ConnectionStatusDialog
  bind:open={connectionStatusDialog.open}
  agent={ws.agent}
  agentState={ws.agent === null ? null : hub.displayStateOf(ws.agent)}
  hubConnection={hub.transport.status}
  agentConnection={ws.transport.status}
/>
<InstallHelpDialog bind:open={installHelp.open} />
<FeedbackDialog bind:open={feedbackOpen} bind:tab={feedbackTab} />
<InboxNoteDialog
  agent={inboxNoteAgent}
  onadd={addInboxNote}
  onclose={() => (inboxNoteAgent = null)}
/>
<ConfirmHost />

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--layout-rail-width) minmax(0, 1fr) auto;
    height: 100%;
    /* Under the status bar, a notch and the home indicator the base surface shows, and the content sits inside. */
    padding: var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left);
    background: var(--color-stone-0);
  }

  .shell-side {
    display: flex;
    min-height: 0;
    border-right: 1px solid var(--color-line-soft);
  }

  .shell-main {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  /* Over everything but toasts, while Settings or the palette waits for its code. */
  .shell-opening {
    position: fixed;
    inset: 0;
    z-index: var(--z-overlay);
    display: grid;
    place-items: center;
    background: transparent;
    color: var(--color-text-2);
    opacity: 0;
    pointer-events: none;
    animation: shell-opening-in var(--duration-base) var(--ease-out) calc(var(--duration-base) * 2)
      forwards;
  }

  @keyframes shell-opening-in {
    to {
      opacity: 1;
    }
  }

  @media (max-width: 760px) {
    .shell {
      grid-template-columns: minmax(0, 1fr);
      /* The bar covers the bottom inset, so the content only clears the bar. */
      padding-bottom: var(--layout-bottom-bar-offset);
    }

    .shell-side {
      display: none;
    }
  }
</style>
