<script lang="ts">
  import { onMount, tick } from "svelte";
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
  import { userInbox } from "./lib/inbox.svelte";

  // Below this width the sessions sidebar becomes a drawer over the page.
  const NARROW_QUERY = "(max-width: 900px)";
  const SIDEBAR_PREF_KEY = "residuum-sessions-sidebar";

  let mode = $state<"loading" | "setup" | "running">("loading");
  let activeView = $state<"chat" | "workspace" | "settings">("chat");
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
    if (narrow) {
      drawerOpen = open;
      if (!open)
        void tick().then(() => document.querySelector<HTMLElement>(".sessions-toggle")?.focus());
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
    if (activeView === "settings") activeView = "chat";
  }

  function backToChat() {
    sessions.closeView();
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
    const anyLive = sessions.live.length > 0;
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

  onMount(async () => {
    try {
      const status = await fetchStatus();
      mode = status.mode === "setup" ? "setup" : "running";
    } catch {
      mode = "running";
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
  <Header
    status={ws.transport.status}
    {activeView}
    onOpenChat={() => {
      activeView = "chat";
    }}
    onOpenWorkspace={() => {
      activeView = activeView === "workspace" ? "chat" : "workspace";
    }}
    onOpenSettings={() => {
      activeView = activeView === "settings" ? "chat" : "settings";
    }}
    onOpenFeedback={() => openFeedback("bug")}
    onOpenInbox={() => {
      inboxOpen = true;
    }}
    sessionsToggle={activeView === "settings"
      ? undefined
      : {
          open: sidebarOpen,
          liveCount: sessions.live.length,
          onToggle: () => setSidebarOpen(!sidebarOpen),
        }}
  />
  {#if activeView === "settings"}
    <Settings
      onClose={() => {
        activeView = "chat";
      }}
    />
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
      <div class="app-main emerges" class:with-workspace={activeView === "workspace"}>
        <div class="workspace-slot" aria-hidden={activeView !== "workspace"}>
          {#if workspaceMounted}
            <Workspace
              onClose={() => {
                activeView = "chat";
              }}
            />
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
