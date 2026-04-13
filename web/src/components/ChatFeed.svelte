<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import type { FeedItem } from "../lib/types";
  import { ws } from "../lib/ws.svelte";
  import { fetchChatSegment } from "../lib/api";
  import { notifications } from "../lib/notifications.svelte";
  import MessageUser from "./MessageUser.svelte";
  import MessageAssistant from "./MessageAssistant.svelte";
  import MessageDivider from "./MessageDivider.svelte";
  import MessageLocalSystem from "./MessageLocalSystem.svelte";
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
  let dividerObserver: MutationObserver | undefined;
  let cachedDividers: HTMLElement[] = [];
  let lastTailId: number | undefined;

  // Anchor pill: visible when the user has scrolled up from the bottom.
  const ANCHOR_THRESHOLD_PX = 400;
  let scrolledUp = $state(false);
  let anchorLabel = $state("");

  function refreshDividerCache() {
    if (!feedEl) {
      cachedDividers = [];
      return;
    }
    cachedDividers = Array.from(
      feedEl.querySelectorAll<HTMLElement>(".msg-divider .msg-divider-label"),
    );
  }

  function scrollToBottom() {
    if (!feedEl) return;
    window.requestAnimationFrame(() => {
      if (feedEl) feedEl.scrollTop = feedEl.scrollHeight;
    });
  }

  function jumpToLatest() {
    if (!feedEl) return;
    feedEl.scrollTo({ top: feedEl.scrollHeight, behavior: "smooth" });
  }

  function updateAnchor() {
    if (!feedEl) return;
    const distFromBottom = feedEl.scrollHeight - feedEl.scrollTop - feedEl.clientHeight;
    scrolledUp = distFromBottom > ANCHOR_THRESHOLD_PX;
    if (!scrolledUp) {
      anchorLabel = "";
      return;
    }
    // Topmost visible divider — its label becomes the anchor.
    // Iterates over the cached divider list (refreshed on DOM mutations).
    const feedTop = feedEl.getBoundingClientRect().top;
    let label = "";
    for (const div of cachedDividers) {
      const top = div.getBoundingClientRect().top;
      if (top >= feedTop) {
        label = div.textContent?.trim() ?? "";
        break;
      }
      // The last divider above the viewport is also a valid candidate
      // until we find one inside it.
      label = div.textContent?.trim() ?? "";
    }
    anchorLabel = label;
  }

  // Match the observer's 200px rootMargin so state-driven and scroll-driven
  // triggers agree on "close enough to load more".
  const LOAD_OLDER_SENTINEL_MARGIN_PX = 200;

  function isSentinelNearView(): boolean {
    if (!feedEl || !topSentinel) return false;
    const sentinelRect = topSentinel.getBoundingClientRect();
    const feedRect = feedEl.getBoundingClientRect();
    return (
      sentinelRect.bottom >= feedRect.top - LOAD_OLDER_SENTINEL_MARGIN_PX &&
      sentinelRect.top <= feedRect.bottom
    );
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
    let prependFailed = false;
    try {
      const segment = await fetchChatSegment(cursor);
      ws.prependEpisode(segment);
    } catch (err) {
      prependFailed = true;
      notifications.surface(
        "error",
        `Couldn't load episode ${cursor}: ${err instanceof Error ? err.message : String(err)}`,
      );
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

    // When the feed is short enough to fit in the viewport, the sentinel
    // stays visible after a prepend, so the IntersectionObserver's
    // intersection state never *changes* and no further callback fires.
    // Chain another load here so we keep pulling episodes until either
    // the sentinel is pushed offscreen or hasMoreHistory is exhausted.
    // The `!prependFailed` guard stops the chain on errors so we don't
    // loop firing the same notification repeatedly.
    if (!prependFailed && isSentinelNearView()) {
      void loadOlder();
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

    // Cache divider elements; refresh only when feed children change.
    refreshDividerCache();
    const inner = feedEl.querySelector(".chat-feed-inner");
    if (inner) {
      dividerObserver = new MutationObserver(refreshDividerCache);
      dividerObserver.observe(inner, { childList: true, subtree: false });
    }

    feedEl.addEventListener("scroll", updateAnchor, { passive: true });
    updateAnchor();
  });

  onDestroy(() => {
    observer?.disconnect();
    dividerObserver?.disconnect();
    feedEl?.removeEventListener("scroll", updateAnchor);
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

  $effect(() => {
    // The IntersectionObserver fires once when the sentinel first mounts
    // — which is BEFORE Chat.svelte's async fetchChatHistory resolves, so
    // `hasMoreHistory` is still false and loadOlder no-ops. Once the
    // initial load flips hasMoreHistory to true, nothing retriggers the
    // observer (the sentinel's intersection state hasn't changed), so the
    // lazy chain silently stalls. React to the state transition directly
    // and kick loadOlder whenever the guards would allow it and the
    // sentinel is still near the viewport.
    void ws.store.hasMoreHistory;
    void ws.store.oldestEpisodeCursor;
    void ws.store.isLoadingOlder;
    void tick().then(() => {
      if (isSentinelNearView()) void loadOlder();
    });
  });
</script>

<div class="chat-feed" bind:this={feedEl}>
  {#if scrolledUp}
    <div class="anchor-pill">
      {#if anchorLabel}
        <span class="anchor-pill-label">{anchorLabel}</span>
        <span class="anchor-pill-divider"></span>
      {/if}
      <button type="button" class="anchor-pill-jump" onclick={jumpToLatest}>
        Jump to latest
      </button>
    </div>
  {/if}
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
      {:else if item.kind === "divider"}
        <MessageDivider label={item.label} variant={item.variant ?? "day"} />
      {:else if item.kind === "compressed-marker"}
        <CompressedHistoryMarker />
      {:else if item.kind === "tool-group"}
        <ToolGroup calls={item.calls} {verbose} />
      {:else if item.kind === "file-attachment"}
        <FileAttachment {item} />
      {:else if item.kind === "local-system"}
        <MessageLocalSystem content={item.content} />
      {/if}
    {/each}
    {#if isProcessing}
      <ThinkingIndicator />
    {/if}
  </div>
</div>
