<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
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
  import TeamView from "./components/TeamView.svelte";
  import { userInbox } from "./lib/inbox.svelte";
  import { hub } from "./lib/hub.svelte";
  import { notifications } from "./lib/notifications.svelte";
  import { userErrorMessage } from "./lib/errors";
  import { router } from "./lib/router.svelte";
  import { legacyRouter } from "./lib/legacy-router.svelte";

  // Below this width the sessions sidebar becomes a drawer over the page.
  const NARROW_QUERY = "(max-width: 900px)";
  const SIDEBAR_PREF_KEY = "residuum-sessions-sidebar";

  let mode = $state<"loading" | "setup" | "running">("loading");

  router.start();

  const activeView = $derived(legacyRouter.view);
  let workspaceMounted = $state(false);
  let helpOpen = $state(false);
  let feedbackOpen = $state(false);
  let feedbackTab = $state<"bug" | "feedback">("bug");
  let narrow = $state(window.matchMedia(NARROW_QUERY).matches);
  let sidebarPreferredOpen = $state(readSidebarPref());
  let drawerOpen = $state(false);

  // The sessions list is the Activity place, so it shows there whatever the preference.
  let sidebarOpen = $derived(legacyRouter.activity || (narrow ? drawerOpen : sidebarPreferredOpen));
  const sessions = $derived(ws.sessions);

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
      if (legacyRouter.activity) {
        legacyRouter.openMainChat();
        return;
      }
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
    legacyRouter.openSession(runId);
    if (narrow) drawerOpen = false;
  }

  function backToChat() {
    legacyRouter.openMainChat();
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
    const runId = legacyRouter.chat.runId;
    untrack(() => {
      if (runId === null) sessions.closeView();
      else sessions.showRun(runId);
    });
  });

  // A session that continues in a new run takes the location with it.
  $effect(() => {
    const followed = sessions.view?.runId;
    if (followed === undefined) return;
    untrack(() => {
      if (legacyRouter.chat.runId !== followed) legacyRouter.replaceSession(followed);
    });
  });

  // No agents yet means first-run setup. The hub socket stays up for the life
  // of the page and feeds the switcher; the agent connection follows the
  // router's viewed agent (see `ws.svelte.ts`), so navigating between pages
  // or to the team never drops it.
  onMount(() => {
    hub.connect();
    void (async () => {
      try {
        await hub.refresh(true);
        mode = hub.agents.length === 0 ? "setup" : "running";
      } catch (err) {
        notifications.surface(
          "error",
          userErrorMessage(err, { action: "Couldn't load your agents." }),
        );
        mode = "running";
      }
    })();
    return () => {
      hub.disconnect();
      ws.disconnect();
    };
  });

  // Settle on agents that exist: a URL on an agent that doesn't goes to Home,
  // and one that resolves under the last-used agent finds it.
  $effect(() => {
    if (mode !== "running" || !hub.loaded) return;
    const names = hub.agents.map((agent) => agent.name);
    untrack(() => router.setKnownAgents(names));
  });

  async function finishSetup() {
    try {
      await hub.refresh(true);
    } catch {
      // the hub socket delivers the list once it connects
    }
    mode = "running";
  }

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
  <Setup onComplete={() => void finishSetup()} />
{:else}
  <!-- A workbench artifact in full view fills the window on its own. -->
  {#if !legacyRouter.workbench?.full}
    <Header
      status={ws.transport.status}
      {activeView}
      onOpenChat={() => legacyRouter.setWorkspace(false)}
      onOpenWorkspace={() => legacyRouter.setWorkspace(activeView !== "workspace")}
      onOpenSettings={() => {
        if (activeView === "settings") legacyRouter.closeSettings();
        else legacyRouter.openSettings();
      }}
      onOpenTeam={() => {
        if (activeView === "team") legacyRouter.closeTeam();
        else legacyRouter.openTeam("overview");
      }}
      onOpenTeamFiles={() => {
        if (activeView === "team-files") legacyRouter.closeTeam();
        else legacyRouter.openTeam("files");
      }}
      onOpenHubSettings={() => {
        if (activeView === "hub-settings") legacyRouter.closeSettings();
        else legacyRouter.openSettings(undefined, "hub");
      }}
      onOpenWorkbench={() => {
        if (activeView === "workbench") legacyRouter.closeWorkbench();
        else legacyRouter.openWorkbench();
      }}
      onOpenScheduled={() => {
        if (activeView === "scheduled") legacyRouter.closeScheduled();
        else legacyRouter.openScheduled();
      }}
      onOpenFeedback={() => openFeedback("bug")}
      onOpenInbox={() => {
        legacyRouter.openInbox();
      }}
      sessionsToggle={activeView === "settings" ||
      activeView === "hub-settings" ||
      activeView === "workbench" ||
      activeView === "scheduled" ||
      activeView === "team" ||
      activeView === "team-files"
        ? undefined
        : {
            open: sidebarOpen,
            liveCount: sessions.live.length + sessions.outbound.length,
            onToggle: () => setSidebarOpen(!sidebarOpen),
          }}
    />
  {/if}
  {#key legacyRouter.agent}
    {#if activeView === "settings" || activeView === "hub-settings"}
      {#key `${legacyRouter.settings?.scope}/${legacyRouter.settingsAgent}`}
        <Settings
          scope={legacyRouter.settings?.scope ?? "agent"}
          agent={legacyRouter.settingsAgent}
          section={legacyRouter.settings?.section ?? "runtime"}
          onSelectSection={(section) => legacyRouter.selectSettingsSection(section)}
          onClose={() => legacyRouter.closeSettings()}
        />
      {/key}
    {:else if activeView === "team"}
      <TeamView onClose={() => legacyRouter.closeTeam()} />
    {:else if activeView === "team-files"}
      <div class="app-body">
        <div class="app-main">
          <Workspace agent={null} scope="team" onClose={() => legacyRouter.closeTeam()} />
        </div>
      </div>
    {:else if activeView === "workbench"}
      <Workbench
        artifact={legacyRouter.workbench?.artifact ?? null}
        full={legacyRouter.workbench?.full ?? false}
        onClose={() => legacyRouter.closeWorkbench()}
      />
    {:else if activeView === "scheduled"}
      <Scheduled onClose={() => legacyRouter.closeScheduled()} />
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
              <Workspace
                agent={legacyRouter.agent}
                onClose={() => legacyRouter.setWorkspace(false)}
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
  {/key}
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
  open={legacyRouter.inbox}
  onClose={() => {
    legacyRouter.closeInbox();
  }}
/>
