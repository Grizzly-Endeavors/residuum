<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { fetchStatus } from "./lib/api";
  import { ws } from "./lib/ws.svelte";
  import Header from "./components/Header.svelte";
  import BrandMark from "./components/BrandMark.svelte";
  import NotificationCorner from "./components/NotificationCorner.svelte";
  import HelpOverlay from "./components/HelpOverlay.svelte";
  import FeedbackModal from "./components/FeedbackModal.svelte";
  import Chat from "./Chat.svelte";
  import Setup from "./Setup.svelte";
  import Settings from "./Settings.svelte";
  import Workspace from "./components/Workspace.svelte";
  import UserInboxDrawer from "./components/UserInboxDrawer.svelte";
  import SessionsSidebar from "./components/SessionsSidebar.svelte";
  import SessionView from "./components/SessionView.svelte";
  import Workbench from "./components/Workbench.svelte";
  import Scheduled from "./Scheduled.svelte";
  import { userInbox } from "./lib/inbox.svelte";
  import { router } from "./lib/router.svelte";

  // Below this width the sessions sidebar becomes a drawer over the page.
  const NARROW_QUERY = "(max-width: 900px)";
  const SIDEBAR_PREF_KEY = "residuum-sessions-sidebar";

  let mode = $state<"loading" | "setup" | "running">("loading");

  // A native OS notification's "Open" action (see the macOS/Windows bridges
  // in src/notify/) points here for a batch summary, since there's no
  // single item to deep-link to — read before `router.start()` replaces an
  // unrecognized path, and open the inbox to the full list once running.
  const openedFromNotificationSummary = window.location.pathname.startsWith("/notification");

  router.start();

  let activeView = $derived.by<"chat" | "workspace" | "settings" | "workbench" | "scheduled">(
    () => {
      if (router.settings !== null) return "settings";
      if (router.workbench !== null) return "workbench";
      if (router.scheduled) return "scheduled";
      return router.chat.workspace ? "workspace" : "chat";
    },
  );
  let workspaceMounted = $state(false);
  let helpOpen = $state(false);
  let feedbackOpen = $state(false);
  let inboxOpen = $state(false);
  let feedbackTab = $state<"bug" | "feedback">("bug");
  let narrow = $state(window.matchMedia(NARROW_QUERY).matches);
  let sidebarPreferredOpen = $state(readSidebarPref());
  let drawerOpen = $state(false);

  let sidebarOpen = $derived(narrow ? drawerOpen : sidebarPreferredOpen);
  const sessions = ws.sessions;

  function readSidebarPref(): boolean {
    try {
      return localStorage.getItem(SIDEBAR_PREF_KEY) !== "closed";
    } catch {
      return true;
    }
  }

  function setSidebarOpen(open: boolean) {
    // Closing removes the focused control, so hand focus back to the toggle.
    if (!open) {
      void tick().then(() => document.querySelector<HTMLElement>(".sessions-toggle")?.focus());
    }
    if (narrow) {
      drawerOpen = open;
      return;
    }
    sidebarPreferredOpen = open;
    try {
      localStorage.setItem(SIDEBAR_PREF_KEY, open ? "open" : "closed");
    } catch {
      // localStorage unavailable
    }
  }

  function selectSession(runId: string) {
    sessions.openRun(runId);
    if (narrow) drawerOpen = false;
  }

  function backToChat() {
    router.openMainChat();
    void tick().then(() =>
      document.querySelector<HTMLTextAreaElement>(".chat-view .chat-input")?.focus(),
    );
  }

  $effect(() => {
    const query = window.matchMedia(NARROW_QUERY);
    const update = () => {
      narrow = query.matches;
      drawerOpen = false;
    };
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  });

  // Tick the clock behind elapsed times only while something is live.
  $effect(() => {
    const anyLive = sessions.live.length + sessions.outbound.length > 0;
    if (!anyLive) return;
    sessions.now = Date.now();
    const timer = window.setInterval(() => {
      sessions.now = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });

  function openFeedback(tab: "bug" | "feedback") {
    feedbackTab = tab;
    feedbackOpen = true;
  }

  $effect(() => {
    if (activeView === "workspace") workspaceMounted = true;
  });

  // The location decides which run the main pane shows.
  $effect(() => {
    const runId = router.chat.runId;
    untrack(() => {
      if (runId === null) sessions.closeView();
      else sessions.showRun(runId);
    });
  });

  onMount(async () => {
    try {
      const status = await fetchStatus();
      mode = status.mode === "setup" ? "setup" : "running";
    } catch {
      mode = "running";
    }
    if (openedFromNotificationSummary) {
      inboxOpen = true;
    }
  });

  // WS lives as long as the page does — it is not tied to any single screen.
  // Navigating to Settings / Workspace must not drop the connection.
  $effect(() => {
    if (mode !== "running") return;
    ws.connect();
    return () => ws.disconnect();
  });

  $effect(() => {
    if (mode === "running") {
      userInbox.startPolling();
      return () => userInbox.stopPolling();
    }
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && narrow && drawerOpen) {
      event.preventDefault();
      setSidebarOpen(false);
      return;
    }
    // `?` opens help — but only when nothing else is taking text input.
    if (event.key !== "?") return;
    const target = event.target as HTMLElement | null;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target?.isContentEditable)
      return;
    event.preventDefault();
    helpOpen = true;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if mode === "loading"}
  <div class="header">
    <div class="header-brand">
      <BrandMark size={26} />
      <span class="header-title">Residuum</span>
      <span class="header-status connecting">loading</span>
    </div>
  </div>
{:else if mode === "setup"}
  <div class="header">
    <div class="header-brand">
      <BrandMark size={26} />
      <span class="header-title">Residuum</span>
    </div>
  </div>
  <Setup
    onComplete={() => {
      mode = "running";
    }}
  />
{:else}
  <!-- A workbench artifact in full view fills the window on its own. -->
  {#if !router.workbench?.full}
    <Header
      status={ws.transport.status}
      {activeView}
      onOpenChat={() => router.setWorkspace(false)}
      onOpenWorkspace={() => router.setWorkspace(activeView !== "workspace")}
      onOpenSettings={() => {
        if (activeView === "settings") router.closeSettings();
        else router.openSettings();
      }}
      onOpenWorkbench={() => {
        if (activeView === "workbench") router.closeWorkbench();
        else router.openWorkbench();
      }}
      onOpenScheduled={() => {
        if (activeView === "scheduled") router.closeScheduled();
        else router.openScheduled();
      }}
      onOpenFeedback={() => openFeedback("bug")}
      onOpenInbox={() => {
        inboxOpen = true;
      }}
      sessionsToggle={activeView === "settings" ||
      activeView === "workbench" ||
      activeView === "scheduled"
        ? undefined
        : {
            open: sidebarOpen,
            liveCount: sessions.live.length + sessions.outbound.length,
            onToggle: () => setSidebarOpen(!sidebarOpen),
          }}
    />
  {/if}
  {#if activeView === "settings"}
    <Settings
      section={router.settings ?? "runtime"}
      onSelectSection={(section) => router.openSettings(section)}
      onClose={() => router.closeSettings()}
    />
  {:else if activeView === "workbench"}
    <Workbench
      artifact={router.workbench?.artifact ?? null}
      full={router.workbench?.full ?? false}
      onClose={() => router.closeWorkbench()}
    />
  {:else if activeView === "scheduled"}
    <Scheduled onClose={() => router.closeScheduled()} />
  {:else}
    <div class="app-body">
      {#if sidebarOpen}
        <SessionsSidebar
          overlay={narrow}
          onClose={() => setSidebarOpen(false)}
          onSelect={selectSession}
        />
        {#if narrow}
          <button
            type="button"
            class="sessions-backdrop"
            aria-label="Close sessions"
            tabindex="-1"
            onclick={() => setSidebarOpen(false)}
          ></button>
        {/if}
      {/if}
      <!-- Behind the open drawer, the page is inert: no focus, no clicks,
           hidden from assistive tech, so the drawer behaves as a modal. -->
      <div
        class="app-main emerges"
        class:with-workspace={activeView === "workspace"}
        inert={narrow && drawerOpen}
      >
        <div class="workspace-slot" aria-hidden={activeView !== "workspace"}>
          {#if workspaceMounted}
            <Workspace onClose={() => router.setWorkspace(false)} />
          {/if}
        </div>
        <div class="main-pane">
          <!-- The chat stays mounted under a session view so its history,
               scroll position, and draft survive a visit to a session. -->
          <div class="chat-slot" class:is-hidden={sessions.view !== null}>
            <Chat onOpenFeedback={() => openFeedback("feedback")} />
          </div>
          {#if sessions.view}
            <SessionView view={sessions.view} onBack={backToChat} />
          {/if}
        </div>
      </div>
    </div>
  {/if}
{/if}

<FeedbackModal
  open={feedbackOpen}
  initialTab={feedbackTab}
  onClose={() => {
    feedbackOpen = false;
  }}
/>

<NotificationCorner />
<HelpOverlay
  open={helpOpen}
  onClose={() => {
    helpOpen = false;
  }}
/>
<UserInboxDrawer
  open={inboxOpen}
  onClose={() => {
    inboxOpen = false;
  }}
/>
