<script lang="ts">
  import { untrack } from "svelte";
  import { actionRegistry, commandActions } from "../../lib/action-registry.svelte";
  import { runCommandLine } from "../../lib/command-line";
  import { composerClearance } from "../../lib/composer-clearance.svelte";
  import { carriesFiles } from "../../lib/file-drop";
  import { hub } from "../../lib/hub.svelte";
  import { Icon } from "../../lib/icons";
  import { startSession } from "../../lib/api";
  import { forkIntoSession } from "../../lib/chat-actions";
  import { userErrorMessage } from "../../lib/errors";
  import { notifications } from "../../lib/notifications.svelte";
  import { openSessionByAddress } from "../../lib/session-address";
  import type { StagedAttachment } from "../../lib/staged-attachments.svelte";
  import type { WaitingMessage } from "../../lib/types";
  import { Button, EmptyState, Skeleton } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import Feed from "../../feed/Feed.svelte";
  import type { FeedHistory } from "../../feed/feed-history";
  import type { ShellActions } from "../../shell/shell-actions";
  import ChatHeader from "./ChatHeader.svelte";
  import Composer from "./Composer.svelte";
  import PostTurnStatus from "./PostTurnStatus.svelte";
  import StateCard from "./StateCard.svelte";
  import WaitingMessages from "./WaitingMessages.svelte";

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

  function handleSend(text: string, attachments: StagedAttachment[]): boolean {
    if (attachments.length > 0) {
      ws.sendChat(text, attachments);
      return true;
    }
    const outcome = runCommandLine(commandActions(actionRegistry.all), text);
    if (outcome === "message") ws.sendChat(text);
    return outcome !== "refused";
  }

  $effect(() => {
    const shown = composer;
    if (shown === undefined) return;
    return ws.onRefusedSend((refused) => {
      shown.restore(refused.content, refused.attachments);
    });
  });

  async function multitask(message: WaitingMessage): Promise<void> {
    const name = hub.shownName(agent);
    if (message.images !== undefined && message.images.length > 0) {
      notifications.surface(
        "notice",
        `A multitask session can't take pictures. Send this now, or let ${name} read it.`,
      );
      return;
    }
    if (message.attachments !== undefined && message.attachments.length > 0) {
      notifications.surface(
        "notice",
        `A multitask session can't take files. Send this now, or let ${name} read it.`,
      );
      return;
    }
    let withdrawn: boolean;
    try {
      withdrawn = await ws.withdrawWaiting(message.id);
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, { action: "Couldn't move the message to a multitask session." }),
      );
      return;
    }
    if (!withdrawn) {
      notifications.surface("notice", `${name} already read that message, so it stays here.`);
      return;
    }
    const started = await forkIntoSession(
      {
        surface: (kind, text, action) => {
          notifications.surface(kind, text, undefined, action);
        },
        startSession,
        openSession: (owner, address) => {
          void openSessionByAddress(owner, address, null);
        },
      },
      agent,
      message.content,
    );
    if (!started) composer?.restore(message.content);
  }

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
    composer?.attach(event.dataTransfer?.files ?? []);
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
        <WaitingMessages
          messages={store.waiting}
          reader={hub.shownName(agent)}
          actions={{
            offline: ws.transport.status !== "connected",
            onsendnow: (message) => {
              ws.sendWaitingNow(message.id);
            },
            onmultitask: (message) => void multitask(message),
          }}
        />
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
      <span class="chat-drop-label"><Icon name="paperclip" size={16} />Drop files to attach</span>
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

  .chat-loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-18);
  }

  .chat-composer {
    padding: var(--space-8) var(--space-24) var(--space-18);
  }

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
