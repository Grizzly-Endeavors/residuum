<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { ws } from "./lib/ws.svelte";
  import { fetchChatHistory } from "./lib/api";
  import { parseCommand } from "./lib/commands";
  import ChatFeed from "./components/ChatFeed.svelte";
  import ChatInput from "./components/ChatInput.svelte";

  let feedIdCounter = 0;
  function nextId(): number {
    return --feedIdCounter; // negative IDs to avoid collision with ws module
  }

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

  function handleSend(text: string, images?: import("./lib/types").ImageAttachment[]) {
    const result = parseCommand(text, {
      connectionStatus: ws.status,
      verbose: ws.verbose,
      nextId,
    });

    if (result) {
      if (result.feedItem) ws.appendFeedItem(result.feedItem);
      if (result.wsMessage) ws.send(result.wsMessage);

      // Handle verbose toggle specially
      if (text.toLowerCase() === "/verbose") {
        ws.setVerbose(!ws.verbose);
      }
      return;
    }

    ws.sendChat(text, images);
  }
</script>

<div class="chat-view">
  <ChatFeed items={ws.feed} isProcessing={ws.isProcessing} verbose={ws.verbose} />
  <ChatInput onSend={handleSend} disabled={ws.status !== "connected"} />
</div>
