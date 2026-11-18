<script lang="ts">
  import { router } from "../lib/router.svelte";
  import Activity from "../places/activity/Activity.svelte";
  import ChatPlace from "../places/chat/ChatPlace.svelte";
  import FilesPlace from "../places/files/FilesPlace.svelte";
  import { fileSourceFor } from "../places/files/file-source";
  import Home from "../places/home/Home.svelte";
  import Inbox from "../places/inbox/Inbox.svelte";
  import NotFound from "../places/not-found/NotFound.svelte";
  import Schedule from "../places/schedule/Schedule.svelte";
  import Workbench from "../places/workbench/Workbench.svelte";
  import AgentPlaceHeader from "./AgentPlaceHeader.svelte";
  import PlaceHeader from "./PlaceHeader.svelte";
  import type { ShellActions } from "./shell-actions";

  // The main region's place: Home, Inbox, an agent's Chat, Activity, Schedule
  // or Files, the Workbench, Shared files, or Not Found.

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
    <ChatPlace agent={place.agent} {actions} />
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
    <AgentPlaceHeader agent={place.agent} kind="files" />
  {:else}
    <PlaceHeader title="Shared files" />
  {/if}
  {#key `${files.scope}:${files.agent ?? ""}`}
    <FilesPlace source={files} />
  {/key}
{:else if place.kind === "workbench"}
  <Workbench artifact={place.artifact} />
{:else if place.kind === "not-found"}
  <NotFound path={place.path} />
{/if}
