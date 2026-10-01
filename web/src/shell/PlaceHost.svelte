<script lang="ts">
  import { tick, untrack } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { router } from "../lib/router.svelte";
  import { isAgentPlace } from "../lib/routes";
  import { ws } from "../lib/ws.svelte";
  import Chat from "../Chat.svelte";
  import Scheduled from "../Scheduled.svelte";
  import SessionView from "../components/SessionView.svelte";
  import SessionsSidebar from "../components/SessionsSidebar.svelte";
  import TeamView from "../components/TeamView.svelte";
  import UserInbox from "../components/UserInbox.svelte";
  import Workbench from "../components/Workbench.svelte";
  import Workspace from "../components/Workspace.svelte";
  import PlaceHeader from "./PlaceHeader.svelte";
  import { agentPlaceLabel } from "./rail-model";

  // The main region's place. A place that hasn't been rebuilt hosts its
  // legacy view inside a `data-legacy-view` element, where the legacy global
  // styles still apply and the new base styles don't.

  const place = $derived(router.place);
  const sessions = $derived(ws.sessions);

  /** The run a session panel names on its agent's place. It shows over the place, in the main region. */
  const panelRun = $derived.by(() => {
    const { panel } = router;
    if (!isAgentPlace(place) || panel?.kind !== "session" || panel.agent !== place.agent) {
      return null;
    }
    return panel.runId;
  });

  $effect(() => {
    const runId = panelRun;
    untrack(() => {
      if (runId === null) sessions.closeView();
      else sessions.showRun(runId);
    });
  });

  // A session that continues in a new run takes the panel with it.
  $effect(() => {
    const followed = sessions.view?.runId;
    if (followed === undefined) return;
    untrack(() => {
      const { panel } = router;
      if (panel?.kind === "session" && panel.runId !== followed) {
        void router.replacePanel({ ...panel, runId: followed });
      }
    });
  });

  // Tick the clock behind elapsed times only while something is live.
  $effect(() => {
    const live = sessions.live.length + sessions.outbound.length > 0;
    if (!live) return;
    sessions.now = Date.now();
    const timer = window.setInterval(() => {
      sessions.now = Date.now();
    }, 1000);
    return () => window.clearInterval(timer);
  });

  function openRun(runId: string): void {
    if (isAgentPlace(place)) void router.openPanel({ kind: "session", agent: place.agent, runId });
  }

  function closeRun(): void {
    void router.closePanel();
    void tick().then(() =>
      document.querySelector<HTMLTextAreaElement>(".chat-view .chat-input")?.focus(),
    );
  }
</script>

{#if isAgentPlace(place)}
  <PlaceHeader
    title={place.agent}
    agent={place.agent}
    sub={place.kind === "chat" ? hub.agent(place.agent)?.role : agentPlaceLabel(place.kind)}
  />
{:else if place.kind === "inbox"}
  <PlaceHeader title="Inbox" />
{:else if place.kind === "shared-files"}
  <PlaceHeader title="Shared files" />
{/if}

<div class="shell-legacy" data-legacy-view>
  {#if isAgentPlace(place)}
    {#key place.agent}
      {#if sessions.view}
        <SessionView view={sessions.view} onBack={closeRun} />
      {/if}
      <!-- The place stays mounted under a session, so the chat's history, scroll and draft survive it. -->
      <div class="shell-legacy-place" class:is-covered={sessions.view !== null}>
        {#if place.kind === "chat"}
          <Chat />
        {:else if place.kind === "activity"}
          <SessionsSidebar onSelect={openRun} />
        {:else if place.kind === "schedule"}
          <Scheduled />
        {:else}
          <Workspace agent={place.agent} />
        {/if}
      </div>
    {/key}
  {:else if place.kind === "home"}
    <TeamView />
  {:else if place.kind === "inbox"}
    <UserInbox />
  {:else if place.kind === "workbench"}
    <Workbench artifact={place.artifact} />
  {:else}
    <Workspace agent={null} scope="team" />
  {/if}
</div>

<style>
  .shell-legacy,
  .shell-legacy-place {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .shell-legacy-place.is-covered {
    display: none;
  }

  /* The team page scrolls across the whole region, its column centered in it. */
  .shell-legacy > :global(.team-view) {
    max-width: none;
    padding-inline: max(var(--space-16), calc((100% - 960px) / 2));
  }

  /* The sessions list was a sidebar; as the Activity place it takes the region. */
  .shell-legacy-place > :global(.sessions-sidebar) {
    flex: 1;
    width: auto;
    border-right: 0;
  }
</style>
