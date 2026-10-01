<script lang="ts">
  import { router } from "../lib/router.svelte";
  import Workbench from "../components/Workbench.svelte";
  import Activity from "../places/activity/Activity.svelte";
  import ChatPlace from "../places/chat/ChatPlace.svelte";
  import FilesPlace from "../places/files/FilesPlace.svelte";
  import { fileSourceFor } from "../places/files/file-source";
  import Home from "../places/home/Home.svelte";
  import Inbox from "../places/inbox/Inbox.svelte";
  import Schedule from "../places/schedule/Schedule.svelte";
  import PlaceHeader from "./PlaceHeader.svelte";
  import { agentPlaceLabel } from "./rail-model";
  import type { ShellActions } from "./shell-actions";

  // The main region's place: Home, Inbox, an agent's Chat, Activity, Schedule
  // or Files, Shared files, or a place that hasn't been rebuilt, which hosts
  // its legacy view inside a `data-legacy-view` element, where the legacy
  // global styles still apply and the new base styles don't.

  let { actions }: { actions: ShellActions } = $props();

  const place = $derived(router.place);
  const files = $derived(fileSourceFor(place));
</script>

{#if place.kind === "home"}
  <Home {actions} />
{:else if place.kind === "inbox"}
  <Inbox {place} />
{:else if place.kind === "chat"}
  {#key place.agent}
    <ChatPlace agent={place.agent} />
  {/key}
{:else if place.kind === "activity"}
  {#key place.agent}
    <Activity agent={place.agent} />
  {/key}
{:else if place.kind === "schedule"}
  {#key place.agent}
    <Schedule agent={place.agent} />
  {/key}
{:else if files !== null && (place.kind === "files" || place.kind === "shared-files")}
  {#if place.kind === "files"}
    <PlaceHeader title={place.agent} agent={place.agent} sub={agentPlaceLabel("files")} />
  {:else}
    <PlaceHeader title="Shared files" />
  {/if}
  {#key `${files.scope}:${files.agent ?? ""}`}
    <FilesPlace source={files} />
  {/key}
{:else if place.kind === "workbench"}
  <div class="shell-legacy" data-legacy-view>
    <Workbench artifact={place.artifact} />
  </div>
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
</style>
