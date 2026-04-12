<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { ws } from "./lib/ws.svelte";
  import { fetchChatHistory, fetchChatSegment } from "./lib/api";
  import { parseCommand } from "./lib/commands";
  import { nextFeedId } from "./lib/feed-id";
  import ChatFeed from "./components/ChatFeed.svelte";
  import ChatInput from "./components/ChatInput.svelte";
  import type { ImageAttachment } from "./lib/types";

  onMount(async () => {
    try {
      const recent = await fetchChatHistory();
      ws.loadHistory(recent);
      // Always prefetch the most recent episode so the chat is never
      // empty after the observer compresses history, and the
      // "compressed history" marker shows up from the start.
      if (recent.next_cursor) {
        const episode = await fetchChatSegment(recent.next_cursor);
        if (episode?.kind === "episode") {
          ws.prependEpisode(episode);
        }
      }
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
