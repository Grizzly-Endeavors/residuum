<script lang="ts">
  import { onDestroy } from "svelte";
  import { toast } from "../lib/toast.svelte";
  import type { AssistantFeedItem } from "../lib/types";
  import { IconButton, VisuallyHidden } from "../lib/ui";
  import { endpointName } from "./feed-words";
  import Prose from "./Prose.svelte";
  import Timestamp from "./Timestamp.svelte";

  // An agent's reply as unboxed prose. While it streams in, its prose ends in
  // a blinking caret. One cut short by a stop keeps what arrived and says so,
  // and a reply delivered to a chat interface says where it went. Its quiet
  // details, Copy (the reply's Markdown to the clipboard) and when it was
  // sent, take no room of their own: they sit in the margin left of the
  // reply's last line where the conversation has one, and otherwise float
  // over the gap below it. Both wait until the reply is whole.

  let { item, agent }: { item: AssistantFeedItem; agent: string } = $props();

  const COPIED_MS = 2000;

  /** The reply was just copied: the button says so, and a polite status does. */
  let copied = $state(false);
  let reset: ReturnType<typeof setTimeout> | undefined;

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(item.content);
    } catch {
      toast.error("Couldn't copy the reply. Select the text and copy it instead.");
      return;
    }
    copied = true;
    clearTimeout(reset);
    reset = setTimeout(() => {
      copied = false;
    }, COPIED_MS);
  }

  onDestroy(() => {
    clearTimeout(reset);
  });
</script>

<div class="reply" data-streaming={item.streaming ? "" : undefined}>
  <Prose content={item.content} {agent} streaming={item.streaming === true} />
  {#if item.cut !== undefined}
    <p class="reply-note">{item.cut === "stopped" ? "Stopped here" : "Cut off here"}</p>
  {/if}
  {#if item.deliveredTo !== undefined}
    <p class="reply-note">Sent to {endpointName(item.deliveredTo)}</p>
  {/if}
  <!-- Kept while it streams, out of sight and out of reach, so nothing moves when it is done. -->
  <div class="reply-meta" data-message-meta inert={item.streaming === true}>
    <IconButton
      size="sm"
      icon={copied ? "check" : "copy"}
      label={copied ? "Copied" : "Copy reply"}
      onclick={() => void copy()}
    />
    {#if item.timestamp !== undefined}
      <Timestamp timestamp={item.timestamp} />
    {/if}
    <VisuallyHidden><span role="status">{copied ? "Copied" : ""}</span></VisuallyHidden>
  </div>
</div>

<style>
  .reply {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
  }

  .reply-note {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  /* Beside nothing until shown: over the gap under the reply, as a small floating row. It touches
     the reply, so the pointer keeps the hover that shows it on its way from the text to Copy. */
  .reply-meta {
    position: absolute;
    top: 100%;
    left: calc(-1 * var(--space-4));
    z-index: var(--z-sticky);
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-2) var(--space-10) var(--space-2) var(--space-2);
    border-radius: var(--corner-pill);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
    opacity: var(--message-meta, 1);
    pointer-events: var(--message-meta-events, auto);
    transition: opacity var(--duration-fast) var(--ease-out);
  }

  .reply[data-streaming] .reply-meta {
    visibility: hidden;
  }

  /* Where the conversation has a margin wide enough, they sit in it, left of the reply's last line.
     1000px is the reading column (768px) plus room for the details on either side. */
  @container feed (min-width: 1000px) {
    .reply-meta {
      top: auto;
      right: 100%;
      bottom: 0;
      left: auto;
      padding: 0 var(--space-8) 0 0;
      background: none;
      box-shadow: none;
    }
  }
</style>
