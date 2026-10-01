<script lang="ts">
  import { ws } from "./lib/ws.svelte";
  import { actionRegistry, commandActions, readCommandLine } from "./lib/action-registry.svelte";
  import { notifications } from "./lib/notifications.svelte";
  import ChatFeed from "./components/ChatFeed.svelte";
  import ChatInput from "./components/ChatInput.svelte";
  import ChatFooter from "./components/ChatFooter.svelte";
  import type { ImageAttachment } from "./lib/types";

  // A line that starts with `/` runs the chat action it names, with the rest
  // of the line as its text; anything else is a message.
  function handleSend(text: string, images?: ImageAttachment[]) {
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

<div class="chat-view">
  <ChatFeed
    items={ws.store.feed}
    isProcessing={ws.store.isProcessing}
    verbose={ws.verbose}
    turnStartedAt={ws.store.turnStartedAt}
    turnOutputTokens={ws.store.turnOutputTokens}
    turnHasUsage={ws.store.turnHasUsage}
    turnToolCalls={ws.store.turnToolCalls}
  />
  <ChatInput
    onSend={handleSend}
    onStop={() => ws.stop()}
    isProcessing={ws.store.isProcessing}
    reconnecting={ws.transport.status !== "connected"}
    pendingCount={ws.transport.pendingCount}
  />
  <ChatFooter
    usage={ws.store.sessionUsage}
    memoryWorking={ws.store.memoryWorking}
    subconsciousWorking={ws.store.subconsciousWorking}
  />
</div>
