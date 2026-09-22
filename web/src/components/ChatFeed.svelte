<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import type { FeedItem } from "../lib/types";
  import { ws } from "../lib/ws.svelte";
  import { FeedScroller } from "../lib/feed-scroll.svelte";
  import FeedItemView from "./FeedItemView.svelte";
  import ThinkingIndicator from "./ThinkingIndicator.svelte";

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
  let lastLength = 0;
  let lastProcessing = false;
  let detachScroller: (() => void) | undefined;

  // Follows new messages while the reader is at the bottom; the anchor pill
  // shows once they scroll up.
  const scroller = new FeedScroller();
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

  function updateAnchor() {
    if (!feedEl) return;
    if (!scroller.scrolledUp) {
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

    const added = await ws.loadOlderHistory();
    if (!added) return;

    await tick();
    // A reader pinned to the bottom stays there (the scroller handles it);
    // otherwise keep what they were looking at in place.
    if (feedEl && anchor && !scroller.isFollowing) {
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
    // A failed load returned early above, so the chain stops on errors
    // rather than repeating the same notification.
    if (isSentinelNearView()) {
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

    if (inner instanceof HTMLElement) detachScroller = scroller.attach(feedEl, inner);
    feedEl.addEventListener("scroll", updateAnchor, { passive: true });
    updateAnchor();
  });

  onDestroy(() => {
    observer?.disconnect();
    dividerObserver?.disconnect();
    detachScroller?.();
    feedEl?.removeEventListener("scroll", updateAnchor);
  });

  $effect(() => {
    // Keep a reader who is at the bottom there as the feed changes — new
    // live messages at the tail, and episodes prepended at the head (browser
    // scroll anchoring is off, so a prepend would otherwise push them up).
    // Their own new message, or a freshly loaded feed, always scrolls down.
    const tail = items[items.length - 1];
    const tailChanged = tail?.id !== lastTailId;
    const lengthChanged = items.length !== lastLength;
    const force =
      (tailChanged && tail?.kind === "user" && !tail.sender) ||
      (lastTailId === undefined && tail !== undefined);
    // The thinking indicator appearing grows the feed too.
    const processingChanged = isProcessing !== lastProcessing;
    lastTailId = tail?.id;
    lastLength = items.length;
    lastProcessing = isProcessing;
    if (tailChanged || lengthChanged || processingChanged || force) {
      void tick().then(() => {
        scroller.contentChanged(force);
      });
    }
  });

  // A reload replaces every item (new DOM nodes), so a reader who has
  // scrolled up is anchored by content: the topmost visible message is found
  // again by its text and put back where it was — as soon as it's back in the
  // feed, which may be only once the episode it now belongs to has loaded.
  interface ContentAnchor {
    key: string;
    nth: number;
    offset: number;
  }
  let reloadAnchor: ContentAnchor | null = null;
  let seenGeneration = ws.store.generation;

  function messageElements(): HTMLElement[] {
    return feedEl
      ? Array.from(feedEl.querySelectorAll<HTMLElement>(".chat-feed-inner > .msg"))
      : [];
  }

  function contentKey(el: HTMLElement): string {
    return `${el.className}\u0000${el.textContent}`;
  }

  function captureAnchor(): ContentAnchor | null {
    if (!feedEl) return null;
    const feedTop = feedEl.getBoundingClientRect().top;
    const elements = messageElements();
    const topmost = elements.find((el) => el.getBoundingClientRect().bottom > feedTop);
    if (!topmost) return null;
    const key = contentKey(topmost);
    const nth = elements
      .slice(0, elements.indexOf(topmost))
      .filter((el) => contentKey(el) === key).length;
    return { key, nth, offset: topmost.getBoundingClientRect().top - feedTop };
  }

  /** Put the anchored message back in place; false if it isn't in the feed. */
  function restoreAnchor(anchor: ContentAnchor): boolean {
    if (!feedEl) return false;
    const match = messageElements().filter((el) => contentKey(el) === anchor.key)[anchor.nth];
    if (!match) return false;
    const offset = match.getBoundingClientRect().top - feedEl.getBoundingClientRect().top;
    feedEl.scrollTo({ top: feedEl.scrollTop + offset - anchor.offset, behavior: "instant" });
    return true;
  }

  $effect.pre(() => {
    const generation = ws.store.generation;
    if (generation === seenGeneration) return;
    seenGeneration = generation;
    reloadAnchor = scroller.isFollowing ? null : captureAnchor();
    if (reloadAnchor) scroller.hold();
  });

  $effect(() => {
    void items.length;
    const anchor = reloadAnchor;
    if (!anchor) return;
    void tick().then(() => {
      // The reader scrolling by hand meanwhile ends the hunt.
      if (reloadAnchor !== anchor) return;
      if (!scroller.isHeld || restoreAnchor(anchor)) {
        reloadAnchor = null;
        scroller.release();
      }
    });
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
  {#if scroller.scrolledUp}
    <div class="anchor-pill">
      {#if anchorLabel}
        <span class="anchor-pill-label">{anchorLabel}</span>
        <span class="anchor-pill-divider"></span>
      {/if}
      <button type="button" class="anchor-pill-jump" onclick={() => scroller.jumpToLatest()}>
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
    {#if items.length === 0}
      <div class="feed-empty">
        <p class="feed-empty-text">No messages yet.</p>
        <p class="feed-empty-hint">
          Type <span class="feed-empty-key">/help</span> to see available commands, or press
          <span class="feed-empty-key">?</span> anytime for the shortcuts overlay.
        </p>
      </div>
    {/if}
    {#each items as item (item.id)}
      <FeedItemView {item} {verbose} />
    {:else}
      <div class="chat-feed-empty">Nothing here yet — send a message to begin.</div>
    {/each}
    {#if isProcessing}
      <ThinkingIndicator />
    {/if}
  </div>
</div>
