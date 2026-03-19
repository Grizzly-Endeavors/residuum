<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { ws } from "./lib/ws.svelte";
  import { fetchChatHistory } from "./lib/api";
  import { parseCommand } from "./lib/commands";
  import { nextFeedId } from "./lib/feed-id";
  import ChatFeed from "./components/ChatFeed.svelte";
  import ChatInput from "./components/ChatInput.svelte";
  import type { ImageAttachment } from "./lib/types";

  onMount(async () => {
    try {
      const history = await fetchChatHistory();
      ws.loadHistory(history);
    } catch {
      // history unavailable — start with empty feed
    }
    ws.connect();
  });

  onDestroy(() => {
    ws.disconnect();
  });

  function handleSend(text: string, images?: ImageAttachment[]) {
    const result = parseCommand(text, {
      connectionStatus: ws.status,
      verbose: ws.verbose,
      nextId: nextFeedId,
      setVerbose: (enabled) => ws.setVerbose(enabled),
    });

    if (result) {
      if (result.feedItem) ws.appendFeedItem(result.feedItem);
      if (result.wsMessage) ws.send(result.wsMessage);
      result.action?.();
      return;
    }

    ws.sendChat(text, images);
  }
</script>

<div class="chat-view">
  <ChatFeed items={ws.feed} isProcessing={ws.isProcessing} verbose={ws.verbose} />
  <ChatInput onSend={handleSend} disabled={ws.status !== "connected"} />
</div>
