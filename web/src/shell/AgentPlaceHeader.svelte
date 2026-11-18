<script lang="ts">
  import type { Snippet } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { openAgentPlace } from "./open-agent-place";
  import type { AgentPanelKind } from "../lib/routes";
  import { IconButton } from "../lib/ui";
  import PanelHeader from "./panel/PanelHeader.svelte";
  import PlaceHeader from "./PlaceHeader.svelte";
  import { agentPlaceEntry } from "./rail-model";

  // The header of an agent's Activity, Schedule or Files, wherever it shows:
  // the place's title bar on its page, with a button that opens it beside the
  // agent's Chat, or the context panel's header beside Chat, with a button
  // that maximizes it to its page.

  interface Props {
    agent: string;
    kind: AgentPanelKind;
    /** Shown in the context panel beside the agent's Chat, rather than as the page. */
    inPanel?: boolean;
    /** The place's own buttons, ahead of the move. */
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
      onclick={() => void openAgentPlace(agent, kind, "page")}
    />
  {:else}
    <IconButton
      icon="panel"
      label="Open {entry.label} beside chat"
      size="sm"
      onclick={() => void openAgentPlace(agent, kind, "beside-chat")}
    />
  {/if}
{/snippet}

{#if inPanel}
  <PanelHeader icon={entry.icon} kind={label} title={entry.label} actions={controls} />
{:else}
  <PlaceHeader title={label} {agent} sub={entry.label} actions={controls} />
{/if}
