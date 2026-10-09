<script lang="ts">
  import type { AssistantFeedItem } from "../lib/types";
  import { endpointName } from "./feed-words";
  import Prose from "./Prose.svelte";

  // An agent's reply as unboxed prose. While it streams in, it ends in a
  // blinking caret. One cut short by a stop keeps what arrived and says so,
  // and a reply delivered to a chat interface says where it went.

  let { item, agent }: { item: AssistantFeedItem; agent: string } = $props();
</script>

<div class="reply" data-streaming={item.streaming ? "" : undefined}>
  <Prose content={item.content} {agent} />
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

  /* The caret follows the last word of the text so far: the end of the last
     paragraph, or of the last item of a closing list. */
  .reply[data-streaming] :global(.prose > :last-child:not(pre, ul, ol))::after,
  .reply[data-streaming] :global(.prose > :is(ul, ol):last-child > li:last-child)::after {
    content: "";
    display: inline-block;
    width: 2px;
    height: 1.05em;
    margin-left: var(--space-2);
    background: var(--color-vein-bright);
    vertical-align: text-bottom;
    animation: reply-caret var(--duration-blink) var(--ease-blink) infinite;
  }

  @keyframes reply-caret {
    50% {
      opacity: 0;
    }
  }
</style>
