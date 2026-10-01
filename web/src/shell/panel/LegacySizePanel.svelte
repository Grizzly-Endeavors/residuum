<script lang="ts">
  import { EmptyState } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import ChatFooter from "../../components/ChatFooter.svelte";
  import PanelHeader from "./PanelHeader.svelte";

  // The conversation's size in the context panel: the legacy chat footer's
  // figures for the bound agent, which is the agent whose place this is.

  const usage = $derived(ws.store.sessionUsage);
</script>

<PanelHeader icon="memory" title="Conversation size" />
{#if usage === null}
  <div class="context-panel-size">
    <EmptyState>The figures show once the agent has replied in this conversation.</EmptyState>
  </div>
{:else}
  <div class="context-panel-size" data-legacy-view>
    <ChatFooter
      {usage}
      memoryWorking={ws.store.memoryWorking}
      subconsciousWorking={ws.store.subconsciousWorking}
    />
  </div>
{/if}

<style>
  .context-panel-size {
    padding: var(--space-12) var(--space-8);
  }
</style>
