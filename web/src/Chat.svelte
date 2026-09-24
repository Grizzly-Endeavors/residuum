<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "./lib/ws.svelte";
  import { parseCommand } from "./lib/commands";
  import { notifications } from "./lib/notifications.svelte";
  import ChatFeed from "./components/ChatFeed.svelte";
  import ChatInput from "./components/ChatInput.svelte";
  import ChatFooter from "./components/ChatFooter.svelte";
  import type { ImageAttachment } from "./lib/types";

  let { onOpenFeedback }: { onOpenFeedback: () => void } = $props();

  onMount(() => {
    void ws.loadMainHistory();
  });

  function handleSend(text: string, images?: ImageAttachment[]) {
    const result = parseCommand(text, {
      connectionStatus: ws.transport.status,
      verbose: ws.verbose,
      setVerbose: (enabled) => ws.setVerbose(enabled),
      pushInline: (content) => ws.store.pushLocalSystem(content),
      activeTurnId: ws.store.activeTurnId,
    });

    if (result) {
      if (result.notification) {
        notifications.surface(result.notification.kind, result.notification.message);
      }
      if (result.wsMessage) ws.send(result.wsMessage);
      result.action?.();
      return;
    }

    ws.sendChat(text, images);
  }
</script>

<div class="chat-view">
  <ChatFeed
    items={ws.store.feed}
    isProcessing={ws.store.isProcessing}
    verbose={ws.verbose}
    turnStartedAt={ws.store.turnStartedAt}
    turnOutputTokens={ws.store.turnOutputTokens}
    turnHasUsage={ws.store.turnHasUsage}
  />
  <ChatInput
    onSend={handleSend}
    onStop={() => ws.stop()}
    {onOpenFeedback}
    isProcessing={ws.store.isProcessing}
    disabled={ws.transport.status !== "connected"}
  />
  <ChatFooter usage={ws.store.sessionUsage} />
</div>
