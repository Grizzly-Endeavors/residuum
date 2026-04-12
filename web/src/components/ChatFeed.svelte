<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import type { FeedItem } from "../lib/types";
  import { ws } from "../lib/ws.svelte";
  import { fetchChatSegment } from "../lib/api";
  import MessageUser from "./MessageUser.svelte";
  import MessageAssistant from "./MessageAssistant.svelte";
  import MessageBadge from "./MessageBadge.svelte";
  import MessageDivider from "./MessageDivider.svelte";
  import CompressedHistoryMarker from "./CompressedHistoryMarker.svelte";
  import ToolGroup from "./ToolGroup.svelte";
  import ThinkingIndicator from "./ThinkingIndicator.svelte";
  import FileAttachment from "./FileAttachment.svelte";

  let {
    items,
    isProcessing,
    verbose,
  }: {
    items: FeedItem[];
    isProcessing: boolean;
    verbose: boolean;
  } = $props();

  let feedEl: HTMLDivElement | undefined = $state();
  let topSentinel: HTMLDivElement | undefined = $state();
  let observer: IntersectionObserver | undefined;
  let lastTailId: number | undefined;

  function scrollToBottom() {
    if (!feedEl) return;
    window.requestAnimationFrame(() => {
      if (feedEl) feedEl.scrollTop = feedEl.scrollHeight;
    });
  }

  async function loadOlder() {
    if (!feedEl) return;
    if (!ws.store.hasMoreHistory || ws.store.isLoadingOlder) return;
    const cursor = ws.store.oldestEpisodeCursor;
    if (!cursor) return;

    // Anchor-element pattern: pick a stable, persistent DOM node from the
    // existing feed and remember its viewport-relative offset. After the
    // prepend we shift scrollTop so the same node lands at the same visual
    // position. This is more robust than diffing scrollHeight because it
    // doesn't depend on height measurements of the new content (which can
    // change as fonts/images settle) and it works correctly at scrollTop=0
    // where browser scroll anchoring is suppressed.
    const anchor = feedEl.querySelector<HTMLElement>(".chat-feed-inner > .msg");
    const feedTop = feedEl.getBoundingClientRect().top;
    const prevAnchorOffset = anchor ? anchor.getBoundingClientRect().top - feedTop : 0;

    ws.setLoadingOlder(true);
    try {
      const segment = await fetchChatSegment(cursor);
      if (segment?.kind === "episode") {
        ws.prependEpisode(segment);
      }
    } finally {
      ws.setLoadingOlder(false);
    }

    await tick();
    if (feedEl && anchor) {
      const newAnchorOffset =
        anchor.getBoundingClientRect().top - feedEl.getBoundingClientRect().top;
      const target = feedEl.scrollTop + (newAnchorOffset - prevAnchorOffset);
      // `scroll-behavior: smooth` on .chat-feed would animate a plain
      // scrollTop assignment — which is exactly the "jerk" during prepend.
      // Force an instant jump for the anchor restoration.
      feedEl.scrollTo({ top: target, behavior: "instant" });
    }
  }

  onMount(() => {
    if (!topSentinel || !feedEl) return;
    observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            void loadOlder();
          }
        }
      },
      {
        root: feedEl,
        rootMargin: "200px 0px 0px 0px",
        threshold: 0,
      },
    );
    observer.observe(topSentinel);
  });

  onDestroy(() => {
    observer?.disconnect();
  });

  $effect(() => {
    // Only auto-scroll when the tail of the feed changes (a new live
    // message was appended). Prepends from lazy-loading change the head
    // and must not drag the viewport to the bottom.
    const tail = items[items.length - 1];
    const tailId = tail?.id;
    const tailChanged = tailId !== lastTailId;
    lastTailId = tailId;
    void isProcessing; // track processing state for thinking indicator scroll
    if (tailChanged) {
      void tick().then(scrollToBottom);
    }
  });
</script>

<div class="chat-feed" bind:this={feedEl}>
  <div class="chat-feed-inner">
    <div class="feed-top-sentinel" bind:this={topSentinel}></div>
    <div
      class="feed-loading-slot"
      class:is-active={ws.store.isLoadingOlder}
      aria-hidden={!ws.store.isLoadingOlder}
    >
      <span class="feed-loading-older">loading earlier messages…</span>
    </div>
    {#each items as item (item.id)}
      {#if item.kind === "user"}
        <MessageUser content={item.content} images={item.images} />
      {:else if item.kind === "assistant"}
        <MessageAssistant content={item.content} />
      {:else if item.kind === "system"}
        <MessageBadge content={item.content} variant="system" />
      {:else if item.kind === "error"}
        <MessageBadge content={item.content} variant="error" />
      {:else if item.kind === "notice"}
        <MessageBadge content={item.content} variant="notice" />
      {:else if item.kind === "divider"}
        <MessageDivider label={item.label} variant={item.variant ?? "day"} />
      {:else if item.kind === "compressed-marker"}
        <CompressedHistoryMarker />
      {:else if item.kind === "tool-group"}
        <ToolGroup calls={item.calls} {verbose} />
      {:else if item.kind === "file-attachment"}
        <FileAttachment {item} />
      {/if}
    {/each}
    {#if isProcessing}
      <ThinkingIndicator />
    {/if}
  </div>
</div>
