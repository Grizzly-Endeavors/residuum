<script lang="ts">
  import type { Attachment } from "svelte/attachments";
  import type { FeedItem } from "../lib/types";
  import AgentMessageCard from "./AgentMessageCard.svelte";
  import AssistantMessage from "./AssistantMessage.svelte";
  import CompressedMarker from "./CompressedMarker.svelte";
  import FeedDivider from "./FeedDivider.svelte";
  import FileAttachment from "./FileAttachment.svelte";
  import LocalNote from "./LocalNote.svelte";
  import StatusLine from "./StatusLine.svelte";
  import TurnFailure from "./TurnFailure.svelte";
  import UserMessage from "./UserMessage.svelte";

  // One feed item, in the main chat or a session's transcript. `agent` is the
  // agent the conversation belongs to, which its links and Undo act on. Tool
  // calls show at the head of their turn (`FeedTurn`), not one by one here.
  //
  // A message keeps its quiet details (when it was sent, Copy) out of sight
  // until the pointer is over it, keyboard focus is in it, or it has been tapped on a
  // screen with no hover: `--message-meta` is 1 then, and 0 otherwise, for the
  // parts that read it. They stay in the page the whole time, so assistive
  // technology reaches them.

  let { item, agent }: { item: FeedItem; agent: string } = $props();

  /** A tap on a touch screen showed the message's quiet details. */
  let revealed = $state(false);

  /** What a tap is meant for rather than for the message around it. */
  const OWN_TAPS = "a, button, summary, input, textarea, select, audio, video";

  const revealOnTap: Attachment<HTMLElement> = (node) => {
    const onclick = (event: MouseEvent): void => {
      if (typeof window.matchMedia !== "function" || !window.matchMedia("(hover: none)").matches) {
        return;
      }
      if (event.target instanceof Element && event.target.closest(OWN_TAPS) !== null) return;
      // Letting go of a text selection isn't a tap on the message.
      if (window.getSelection()?.isCollapsed === false) return;
      revealed = !revealed;
    };
    node.addEventListener("click", onclick);
    return () => node.removeEventListener("click", onclick);
  };
</script>

<div class="feed-message" data-revealed={revealed ? "" : undefined} {@attach revealOnTap}>
  {#if item.kind === "user"}
    <UserMessage {item} {agent} />
  {:else if item.kind === "assistant"}
    <AssistantMessage {item} {agent} />
  {:else if item.kind === "agent-message"}
    <AgentMessageCard {item} {agent} />
  {:else if item.kind === "divider"}
    <FeedDivider label={item.label} date={item.date} episode={item.episode} />
  {:else if item.kind === "compressed-marker"}
    <CompressedMarker {agent} />
  {:else if item.kind === "file-attachment"}
    <FileAttachment {item} />
  {:else if item.kind === "turn-failure"}
    <TurnFailure {item} {agent} />
  {:else if item.kind === "status"}
    <StatusLine tone={item.tone} content={item.content} details={item.details} />
  {:else if item.kind === "local-system"}
    <LocalNote content={item.content} />
  {/if}
</div>

<style>
  .feed-message {
    --message-meta: 0;

    min-width: 0;

    /* Keyboard focus only: a button pressed with the mouse keeps focus, and shouldn't pin the details open. */
    &:has(:global(:focus-visible)),
    &[data-revealed] {
      --message-meta: 1;
    }
  }

  /* Hover sticks after a tap on a touch screen, so it counts only where a pointer can hover. */
  @media (hover: hover) {
    .feed-message:hover {
      --message-meta: 1;
    }
  }
</style>
