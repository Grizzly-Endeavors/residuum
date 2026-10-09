<script lang="ts">
  import { onDestroy } from "svelte";
  import { toast } from "../lib/toast.svelte";
  import type { AssistantFeedItem } from "../lib/types";
  import { Button, VisuallyHidden } from "../lib/ui";
  import { endpointName } from "./feed-words";
  import Prose from "./Prose.svelte";
  import Timestamp from "./Timestamp.svelte";

  // An agent's reply as unboxed prose. While it streams in, its prose ends in
  // a blinking caret. One cut short by a stop keeps what arrived and says so,
  // and a reply delivered to a chat interface says where it went. Under it, a
  // quiet row holds Copy, which puts the reply's Markdown on the clipboard,
  // and when it was sent. Both wait until the reply is whole.

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
  <!-- Kept while it streams, out of sight and out of reach, so the reply doesn't move when it is done. -->
  <div class="reply-foot" data-message-meta inert={item.streaming === true}>
    <span class="reply-copy">
      <Button
        variant="quiet"
        size="sm"
        icon={copied ? "check" : "copy"}
        aria-label={copied ? undefined : "Copy reply"}
        onclick={() => void copy()}
      >
        {copied ? "Copied" : "Copy"}
      </Button>
    </span>
    {#if item.timestamp !== undefined}
      <Timestamp timestamp={item.timestamp} />
    {/if}
    <VisuallyHidden><span role="status">{copied ? "Copied" : ""}</span></VisuallyHidden>
  </div>
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

  /* Lined up with the text, and hanging into the gap below, so the row takes little height. */
  .reply-foot {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    margin: 0 0 calc(-1 * var(--space-12)) calc(-1 * var(--space-8));
  }

  .reply[data-streaming] .reply-foot {
    visibility: hidden;
  }

  /* Quiet until the message is hovered or focused; a touch screen has neither, so there it stays. */
  .reply-copy {
    opacity: var(--message-meta, 1);
    transition: opacity var(--duration-fast) var(--ease-out);
  }

  @media (hover: none) {
    .reply-copy {
      opacity: 1;
    }
  }
</style>
