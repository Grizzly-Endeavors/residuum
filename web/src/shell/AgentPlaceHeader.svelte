<script lang="ts">
  import type { Snippet } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { router } from "../lib/router.svelte";
  import { agentPlaceLocation, type AgentPanelKind } from "../lib/routes";
  import { IconButton } from "../lib/ui";
  import PanelHeader from "./panel/PanelHeader.svelte";
  import PlaceHeader from "./PlaceHeader.svelte";
  import { agentPlaceEntry } from "./rail-model";

  interface Props {
    agent: string;
    kind: AgentPanelKind;
    inPanel?: boolean;
    actions?: Snippet;
  }

  let { agent, kind, inPanel = false, actions }: Props = $props();

  const entry = $derived(agentPlaceEntry(kind));
  const label = $derived(hub.shownName(agent));
</script>

{#snippet controls()}
  {@render actions?.()}
  {#if inPanel}
    <IconButton
      icon="expand"
      label="Maximize {entry.label}"
      size="sm"
      onclick={() => void router.open(agentPlaceLocation(agent, kind, "page"))}
    />
  {:else}
    <IconButton
      icon="panel"
      label="Open {entry.label} beside chat"
      size="sm"
      onclick={() => void router.open(agentPlaceLocation(agent, kind, "beside-chat"))}
    />
  {/if}
{/snippet}

{#if inPanel}
  <PanelHeader icon={entry.icon} kind={label} title={entry.label} actions={controls} />
{:else}
  <PlaceHeader title={label} {agent} sub={entry.label} actions={controls} />
{/if}
