<script lang="ts">
  import { untrack } from "svelte";
  import {
    actionRegistry,
    commandActions,
    readCommandLine,
  } from "../../lib/action-registry.svelte";
  import { hub } from "../../lib/hub.svelte";
  import { notifications } from "../../lib/notifications.svelte";
  import type { ImageAttachment } from "../../lib/types";
  import { EmptyState } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import ChatFooter from "../../components/ChatFooter.svelte";
  import ChatInput from "../../components/ChatInput.svelte";
  import ThinkingIndicator from "../../components/ThinkingIndicator.svelte";
  import Feed from "../../feed/Feed.svelte";
  import type { FeedHistory } from "../../feed/feed-history";
  import type { ShellActions } from "../../shell/shell-actions";
  import ChatHeader from "./ChatHeader.svelte";
  import StateCard from "./StateCard.svelte";

  // An agent's Chat: its header, the conversation, and under it the composer.
  // The agent is the bound one, so the conversation is the coordinator's
  // feed. While the agent isn't running, its state card takes the
  // composer's place at the end of the conversation. The composer, its
  // footer and the running-turn line are the legacy ones until their units
  // rebuild them.

  let { agent, actions }: { agent: string; actions: ShellActions } = $props();

  const store = $derived(ws.store);
  const summary = $derived(hub.agent(agent));
  const shownState = $derived(hub.displayStateOf(agent));
  const running = $derived(shownState === "running");

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

  // Starting the agent from its card leaves the keyboard on the card, which
  // goes once the agent runs: the composer that replaces it takes over.
  let cardEl = $state<HTMLDivElement>();
  let composerEl = $state<HTMLDivElement>();
  let focusComposer = false;
  $effect.pre(() => {
    if (!running) return;
    untrack(() => {
      focusComposer = cardEl?.contains(document.activeElement) ?? false;
    });
  });
  $effect(() => {
    if (!running || !composerEl) return;
    untrack(() => {
      if (focusComposer) composerEl?.querySelector("textarea")?.focus();
      focusComposer = false;
    });
  });

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
    liveTurnId={store.activeTurnId}
  >
    {#snippet empty()}
      {#if running}
        <div class="chat-empty">
          <EmptyState variant="block" icon="chat" title="No messages yet" headingLevel={2}>
            Tell {agent} what you need. It will ask about anything it's missing.
          </EmptyState>
        </div>
      {/if}
    {/snippet}
    {#snippet tail()}
      {#if running && store.isProcessing}
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
      {#if summary && shownState !== null && shownState !== "running"}
        <div bind:this={cardEl}>
          <StateCard agent={summary} shown={shownState} alone={store.feed.length === 0} {actions} />
        </div>
      {/if}
    {/snippet}
  </Feed>
  {#if running}
    <div class="chat-composer" data-legacy-view bind:this={composerEl}>
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
  {/if}
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
