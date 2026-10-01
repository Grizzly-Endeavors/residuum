<script lang="ts">
  import { router } from "../lib/router.svelte";
  import { isAgentPlace } from "../lib/routes";
  import { ws } from "../lib/ws.svelte";
  import SessionsSidebar from "../components/SessionsSidebar.svelte";
  import Workbench from "../components/Workbench.svelte";
  import ChatPlace from "../places/chat/ChatPlace.svelte";
  import FilesPlace from "../places/files/FilesPlace.svelte";
  import { fileSourceFor } from "../places/files/file-source";
  import Home from "../places/home/Home.svelte";
  import Inbox from "../places/inbox/Inbox.svelte";
  import Schedule from "../places/schedule/Schedule.svelte";
  import PlaceHeader from "./PlaceHeader.svelte";
  import { agentPlaceLabel } from "./rail-model";
  import type { ShellActions } from "./shell-actions";

  // The main region's place: Home, Inbox, an agent's Chat, Schedule or Files,
  // Shared files, or a place that hasn't been rebuilt, which hosts its legacy
  // view inside a `data-legacy-view` element, where the legacy global styles
  // still apply and the new base styles don't.

  let { actions }: { actions: ShellActions } = $props();

  const place = $derived(router.place);
  const files = $derived(fileSourceFor(place));
  const sessions = $derived(ws.sessions);

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
</script>

{#if place.kind === "home"}
  <Home {actions} />
{:else if place.kind === "inbox"}
  <Inbox {place} />
{:else if place.kind === "chat"}
  {#key place.agent}
    <ChatPlace agent={place.agent} />
  {/key}
{:else if place.kind === "schedule"}
  {#key place.agent}
    <Schedule agent={place.agent} />
  {/key}
{:else}
  {#if isAgentPlace(place)}
    <PlaceHeader title={place.agent} agent={place.agent} sub={agentPlaceLabel(place.kind)} />
  {:else if place.kind === "shared-files"}
    <PlaceHeader title="Shared files" />
  {/if}

  {#if files !== null && (place.kind === "files" || place.kind === "shared-files")}
    {#key `${files.scope}:${files.agent ?? ""}`}
      <FilesPlace source={files} />
    {/key}
  {:else}
    <div class="shell-legacy" data-legacy-view>
      {#if isAgentPlace(place)}
        {#key place.agent}
          <SessionsSidebar onSelect={openRun} />
        {/key}
      {:else if place.kind === "workbench"}
        <Workbench artifact={place.artifact} />
      {/if}
    </div>
  {/if}
{/if}

<style>
  .shell-legacy {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  /* The sessions list was a sidebar; as the Activity place it takes the region. */
  .shell-legacy > :global(.sessions-sidebar) {
    flex: 1;
    width: auto;
    border-right: 0;
  }
</style>
