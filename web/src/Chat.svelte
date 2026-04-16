<script lang="ts">
  import { onMount } from "svelte";
  import { ws } from "./lib/ws.svelte";
  import { fetchChatHistory, fetchChatSegment } from "./lib/api";
  import { parseCommand } from "./lib/commands";
  import { notifications } from "./lib/notifications.svelte";
  import ChatFeed from "./components/ChatFeed.svelte";
  import ChatInput from "./components/ChatInput.svelte";
  import type { ImageAttachment } from "./lib/types";

  let { onOpenFeedback }: { onOpenFeedback: () => void } = $props();

  onMount(async () => {
    let recent;
    try {
      recent = await fetchChatHistory();
    } catch (err) {
      notifications.surface(
        "error",
        `Couldn't load chat history: ${err instanceof Error ? err.message : String(err)}`,
      );
      return;
    }
    ws.loadHistory(recent);
    // Always prefetch the most recent episode so the chat is never
    // empty after the observer compresses history, and the
    // "compressed history" marker shows up from the start.
    if (recent.next_cursor) {
      try {
        const episode = await fetchChatSegment(recent.next_cursor);
        ws.prependEpisode(episode);
      } catch (err) {
        notifications.surface(
          "error",
          `Couldn't load episode ${recent.next_cursor}: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }
  });

  function handleSend(text: string, images?: ImageAttachment[]) {
    const result = parseCommand(text, {
      connectionStatus: ws.transport.status,
      verbose: ws.verbose,
      setVerbose: (enabled) => ws.setVerbose(enabled),
      pushInline: (content) => ws.store.pushLocalSystem(content),
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
  <ChatFeed items={ws.store.feed} isProcessing={ws.store.isProcessing} verbose={ws.verbose} />
  <ChatInput onSend={handleSend} {onOpenFeedback} disabled={ws.transport.status !== "connected"} />
</div>
