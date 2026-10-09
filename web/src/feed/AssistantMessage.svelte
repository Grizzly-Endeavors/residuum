<script lang="ts">
  import type { AssistantFeedItem } from "../lib/types";
  import { endpointName } from "./feed-words";
  import Prose from "./Prose.svelte";

  // An agent's reply as unboxed prose. While it streams in, its prose ends in
  // a blinking caret. One cut short by a stop keeps what arrived and says so,
  // and a reply delivered to a chat interface says where it went.

  let { item, agent }: { item: AssistantFeedItem; agent: string } = $props();
</script>

<div class="reply" data-streaming={item.streaming ? "" : undefined}>
  <Prose content={item.content} {agent} streaming={item.streaming === true} />
  {#if item.cut !== undefined}
    <p class="reply-note">{item.cut === "stopped" ? "Stopped here" : "Cut off here"}</p>
  {/if}
  {#if item.deliveredTo !== undefined}
    <p class="reply-note">Sent to {endpointName(item.deliveredTo)}</p>
  {/if}
</div>

<style>
  .reply {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
  }

  .reply-note {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }
</style>
