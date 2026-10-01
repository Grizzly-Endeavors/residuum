<script lang="ts">
  import { onMount, tick } from "svelte";
  import { userInbox } from "../lib/inbox.svelte";
  import { router } from "../lib/router.svelte";
  import { ALL_SCOPE } from "../lib/settings-sections";
  import { ConfirmHost, confirmLeave, Drawer, RecentNotifications } from "../lib/ui";
  import { PHONE_QUERY } from "../styles/breakpoints";
  import FeedbackModal from "../components/FeedbackModal.svelte";
  import HelpOverlay from "../components/HelpOverlay.svelte";
  import { RailAccordion } from "./accordion.svelte";
  import BottomBar from "./BottomBar.svelte";
  import CreateAgentDialog from "./CreateAgentDialog.svelte";
  import HubBanner from "./HubBanner.svelte";
  import PanelHost from "./panel/PanelHost.svelte";
  import PlaceHost from "./PlaceHost.svelte";
  import Rail from "./Rail.svelte";
  import SettingsModal from "./SettingsModal.svelte";
  import type { FeedbackTab, ShellActions } from "./shell-actions";

  // The frame around every place: the rail beside the main region at medium
  // and wide widths, and the context panel beside it (wide) or over it
  // (medium); on phones the bottom bar, with the rail in a drawer and the
  // panel a full-screen sheet. The shell also owns the overlays its controls
  // open.

  router.guard.setConfirm(confirmLeave);

  /** The drawer opens on the row of the place the user is on. */
  const CURRENT_ROW = '[aria-current="page"]';

  const accordion = new RailAccordion();
  let drawerOpen = $state(false);
  let notificationsOpen = $state(false);
  let shortcutsOpen = $state(false);
  let feedbackOpen = $state(false);
  let feedbackTab = $state<FeedbackTab>("bug");
  let createOpen = $state(false);
  let sideRail = $state<HTMLElement>();

  // Arriving on an agent opens its places in the rail.
  $effect(() => {
    accordion.follow(router.viewedAgent);
  });

  const actions: ShellActions = {
    openSettings: () => {
      drawerOpen = false;
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
  };

  /** A new agent's rail row takes focus, where the rail shows beside the main region (not on phones). */
  async function focusCreatedAgent(name: string): Promise<void> {
    await tick();
    sideRail?.querySelector<HTMLElement>(`[data-rail-agent="${CSS.escape(name)}"]`)?.focus();
  }

  onMount(() => {
    userInbox.startPolling();
    // The drawer is the phone's rail; a wider window shows the rail itself.
    const phone = window.matchMedia(PHONE_QUERY);
    const leftPhoneWidth = (): void => {
      if (!phone.matches) drawerOpen = false;
    };
    phone.addEventListener("change", leftPhoneWidth);
    return () => {
      userInbox.stopPolling();
      phone.removeEventListener("change", leftPhoneWidth);
    };
  });

  function handleKeydown(event: KeyboardEvent): void {
    // `?` opens the shortcuts, unless something is taking text.
    if (event.key !== "?" || event.defaultPrevented) return;
    const target = event.target as HTMLElement | null;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target?.isContentEditable) {
      return;
    }
    event.preventDefault();
    shortcutsOpen = true;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="shell" data-ui>
  <div class="shell-side" bind:this={sideRail}>
    <Rail {accordion} {actions} />
  </div>
  <main class="shell-main">
    <HubBanner />
    <PlaceHost {actions} />
  </main>
  <PanelHost />
  <BottomBar {drawerOpen} onmenu={() => (drawerOpen = !drawerOpen)} {actions} />
</div>

<Drawer bind:open={drawerOpen} label="Agents and places" initialFocus={CURRENT_ROW}>
  <Rail {accordion} {actions} onclose={() => (drawerOpen = false)} />
</Drawer>
<SettingsModal />
<CreateAgentDialog bind:open={createOpen} oncreated={(name) => void focusCreatedAgent(name)} />
<RecentNotifications bind:open={notificationsOpen} />
<ConfirmHost />

<!-- Legacy overlays, outside the shell root so the legacy styles reach them. -->
<HelpOverlay open={shortcutsOpen} onClose={() => (shortcutsOpen = false)} />
<FeedbackModal
  open={feedbackOpen}
  initialTab={feedbackTab}
  onClose={() => (feedbackOpen = false)}
/>

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--layout-rail-width) minmax(0, 1fr) auto;
    height: 100%;
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

  @media (max-width: 760px) {
    .shell {
      grid-template-columns: minmax(0, 1fr);
      padding-bottom: var(--layout-bottom-bar-offset);
    }

    .shell-side {
      display: none;
    }
  }
</style>
