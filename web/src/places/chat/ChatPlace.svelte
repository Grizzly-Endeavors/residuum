<script lang="ts">
  import {
    actionRegistry,
    commandActions,
    readCommandLine,
  } from "../../lib/action-registry.svelte";
  import { notifications } from "../../lib/notifications.svelte";
  import type { ImageAttachment } from "../../lib/types";
  import { EmptyState } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import ChatFooter from "../../components/ChatFooter.svelte";
  import ChatInput from "../../components/ChatInput.svelte";
  import ThinkingIndicator from "../../components/ThinkingIndicator.svelte";
  import Feed from "../../feed/Feed.svelte";
  import type { FeedHistory } from "../../feed/feed-history";
  import ChatHeader from "./ChatHeader.svelte";

  // An agent's Chat: its header, the conversation, and under it the composer.
  // The agent is the bound one, so the conversation is the coordinator's
  // feed. The composer, its footer and the running-turn line are the legacy
  // ones until their units rebuild them.

  let { agent }: { agent: string } = $props();

  const store = $derived(ws.store);

  const history: FeedHistory = {
    get hasMore() {
      return ws.store.hasMoreHistory;
    },
    get loadingOlder() {
      return ws.store.isLoadingOlder;
    },
    get generation() {
      return ws.store.generation;
    },
    loadOlder: () => ws.loadOlderHistory(),
  };

  // A line that starts with `/` runs the chat action it names, with the rest
  // of the line as its text; anything else is a message.
  function handleSend(text: string, images?: ImageAttachment[]): void {
    const line = readCommandLine(commandActions(actionRegistry.all), text);
    if (line === null) {
      ws.sendChat(text, images);
      return;
    }
    if (line.action === null) {
      notifications.surface(
        "error",
        `There's no /${line.name}. Type / at the start of a message to see the chat actions.`,
      );
    } else if (line.action.disabled !== undefined) {
      notifications.surface("error", `Couldn't run /${line.name}: ${line.action.disabled}.`);
    } else {
      void actionRegistry.run(line.action, line.text);
    }
  }
</script>

<div class="chat-place">
  <ChatHeader {agent} />
  <Feed
    {agent}
    items={store.feed}
    verbose={ws.verbose}
    label="Conversation with {agent}"
    {history}
    loading={!store.historyLoaded}
    live={store.isProcessing}
  >
    {#snippet empty()}
      <div class="chat-empty">
        <EmptyState variant="block" icon="chat" title="No messages yet" headingLevel={2}>
          Tell {agent} what you need. It will ask about anything it's missing.
        </EmptyState>
      </div>
    {/snippet}
    {#snippet tail()}
      {#if store.isProcessing}
        <div data-legacy-view>
          <ThinkingIndicator
            since={store.turnStartedAt}
            outputTokens={store.turnOutputTokens}
            hasUsage={store.turnHasUsage}
            toolCalls={store.turnToolCalls}
            stopHint="Esc to stop"
          />
        </div>
      {/if}
    {/snippet}
  </Feed>
  <div class="chat-composer" data-legacy-view>
    <ChatInput
      onSend={handleSend}
      onStop={() => ws.stop()}
      isProcessing={store.isProcessing}
      reconnecting={ws.transport.status !== "connected"}
      pendingCount={ws.transport.pendingCount}
    />
    <ChatFooter
      usage={store.sessionUsage}
      memoryWorking={store.memoryWorking}
      subconsciousWorking={store.subconsciousWorking}
    />
  </div>
</div>

<style>
  .chat-place {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .chat-empty {
    display: flex;
    justify-content: center;
  }

  /* The legacy composer floats over its feed; here it sits under the conversation. */
  .chat-composer {
    flex: none;
    padding: 0 var(--space-24) var(--space-8);

    & :global(.chat-input-area) {
      position: static;
      width: auto;
      max-width: var(--layout-reading-width);
      margin: 0 auto;
      transform: none;
    }

    & :global(.chat-footer) {
      max-width: var(--layout-reading-width);
      margin: 0 auto;
    }
  }

  @media (max-width: 760px) {
    .chat-composer {
      padding: 0 var(--space-10) var(--space-4);
    }
  }
</style>
