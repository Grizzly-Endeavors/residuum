<script lang="ts">
  import { untrack } from "svelte";
  import {
    actionRegistry,
    commandActions,
    readCommandLine,
  } from "../../lib/action-registry.svelte";
  import { composerClearance } from "../../lib/composer-clearance.svelte";
  import { carriesFiles } from "../../lib/file-drop";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { notifications } from "../../lib/notifications.svelte";
  import type { ImageAttachment } from "../../lib/types";
  import { Button, EmptyState, Skeleton } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import Feed from "../../feed/Feed.svelte";
  import type { FeedHistory } from "../../feed/feed-history";
  import type { ShellActions } from "../../shell/shell-actions";
  import ChatHeader from "./ChatHeader.svelte";
  import Composer from "./Composer.svelte";
  import PostTurnStatus from "./PostTurnStatus.svelte";
  import StateCard from "./StateCard.svelte";

  // An agent's Chat: its header, the conversation, and the composer floating
  // over the foot of the conversation, whose scrolling area runs behind it
  // to the bottom of the place. The agent is the bound one, so the
  // conversation is the coordinator's feed. While the agent isn't running,
  // its state card takes the composer's place at the end of the conversation.
  // Images dropped anywhere on the place are attached to the message.

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
  let composer = $state<ReturnType<typeof Composer>>();
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

  // A line whose first word is a chat action's `/name` runs that action, with
  // the rest of the line as its text; anything else is a message, a pasted
  // path included. An action that can't run now says why and leaves the line
  // in the box.
  function handleSend(text: string, images?: ImageAttachment[]): boolean {
    const line = readCommandLine(commandActions(actionRegistry.all), text);
    if (line === null) {
      ws.sendChat(text, images);
      return true;
    }
    if (line.action.disabled !== undefined) {
      notifications.surface(
        "error",
        `Couldn't run /${line.action.command ?? ""}: ${line.action.disabled}.`,
      );
      return false;
    }
    void actionRegistry.run(line.action, line.text);
    return true;
  }

  // ── Dropping images ────────────────────────────────────────────────

  // Files dragged anywhere over the place are for the composer, with an
  // overlay saying so. With no composer (the agent isn't running) the place
  // takes none; the app still keeps the browser from opening them.
  let dragging = $state(false);

  function ondragover(event: DragEvent): void {
    if (!running || !carriesFiles(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    dragging = true;
  }

  function ondragleave(event: DragEvent): void {
    const place = event.currentTarget;
    if (place instanceof Node && !place.contains(event.relatedTarget as Node | null)) {
      dragging = false;
    }
  }

  function ondrop(event: DragEvent): void {
    if (!running || !carriesFiles(event)) return;
    event.preventDefault();
    dragging = false;
    void composer?.attach(event.dataTransfer?.files ?? []);
  }

  const watchDrops = (place: HTMLElement): (() => void) => {
    const listeners = { dragover: ondragover, dragleave: ondragleave, drop: ondrop };
    for (const [type, listener] of Object.entries(listeners)) {
      place.addEventListener(type, listener as EventListener);
    }
    return () => {
      for (const [type, listener] of Object.entries(listeners)) {
        place.removeEventListener(type, listener as EventListener);
      }
    };
  };
</script>

{#snippet composerDock()}
  <div class="chat-composer" bind:this={composerEl} {@attach composerClearance.track}>
    <Composer
      bind:this={composer}
      {agent}
      replying={store.activeTurnId !== null}
      reconnecting={ws.transport.lost}
      queued={ws.queuedMessages}
      onsend={handleSend}
      onstop={() => ws.stop()}
    />
  </div>
{/snippet}

<div class="chat-place" {@attach watchDrops}>
  <ChatHeader {agent} />
  <Feed
    {agent}
    items={store.feed}
    label="Conversation with {hub.shownName(agent)}"
    {history}
    loading={!store.historyLoaded}
    live={store.isProcessing}
    liveTurnId={store.activeTurnId}
    observed={store.observed.get}
    onStop={() => ws.stop()}
    announcement={store.announcement}
    dock={running ? composerDock : undefined}
  >
    {#snippet empty()}
      {#if running}
        <div class="chat-empty">
          <EmptyState variant="block" icon="chat" title="No messages yet" headingLevel={2}>
            Tell {hub.shownName(agent)} what you need. It will ask about anything it's missing.
          </EmptyState>
        </div>
      {/if}
    {/snippet}
    {#snippet tail()}
      {#if !store.historyLoaded}
        {#if ws.historyError !== null}
          <div class="chat-empty">
            <EmptyState
              variant="block"
              icon="warning"
              title="Couldn't load the conversation"
              headingLevel={2}
            >
              {ws.historyError}
              {#snippet actions()}
                <Button onclick={() => void ws.loadMainHistory()}>Retry</Button>
              {/snippet}
            </EmptyState>
          </div>
        {:else}
          <div class="chat-loading">
            <Skeleton shape="block" height="56px" width="55%" label="Loading the conversation" />
            <Skeleton lines={3} />
            <Skeleton shape="block" height="40px" width="40%" />
            <Skeleton lines={4} />
          </div>
        {/if}
      {/if}
      {#if running}
        <PostTurnStatus memory={store.memoryWorking} review={store.subconsciousWorking} />
      {:else if summary && shownState !== null && shownState !== "running"}
        <div bind:this={cardEl}>
          <StateCard agent={summary} shown={shownState} alone={store.feed.length === 0} {actions} />
        </div>
      {/if}
    {/snippet}
  </Feed>
  {#if dragging}
    <div class="chat-drop" aria-hidden="true">
      <span class="chat-drop-label"><Icon name="paperclip" size={16} />Drop images to attach</span>
    </div>
  {/if}
</div>

<style>
  .chat-place {
    position: relative;
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

  /* Stands in for the conversation, in the shape of a message and a reply. */
  .chat-loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-18);
  }

  /* The dock supplies the stone-0 behind the composer; this is its room around it. */
  .chat-composer {
    padding: var(--space-8) var(--space-24) var(--space-18);
  }

  /* Over the whole place, composer and header included, while files are dragged over it. */
  .chat-drop {
    position: absolute;
    inset: 0;
    z-index: var(--z-panel);
    display: grid;
    place-items: center;
    background: var(--color-vein-faint);
    box-shadow: inset 0 0 0 2px var(--color-vein);
    pointer-events: none;
  }

  .chat-drop-label {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-10) var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
    color: var(--color-text);
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-medium);

    & :global(svg) {
      color: var(--color-vein-bright);
    }
  }

  @media (max-width: 760px) {
    .chat-composer {
      padding: var(--space-6) var(--space-10) var(--space-10);
    }
  }
</style>
