<script lang="ts">
  import { tick, untrack, type Snippet } from "svelte";
  import { FeedScroller } from "../lib/feed-scroll.svelte";
  import type { ObservedTurnLookup } from "../lib/observed-turns.svelte";
  import type { FeedItem } from "../lib/types";
  import { keyboardScrollable, Spinner, VisuallyHidden } from "../lib/ui";
  import type { FeedHistory } from "./feed-history";
  import FeedItemView from "./FeedItemView.svelte";
  import FeedTurn from "./FeedTurn.svelte";
  import { groupTurns } from "./turns";

  // A conversation in the reading column: the main chat, or a session's
  // transcript, with each turn's output grouped (`turns.ts`). It follows new
  // content while the reader is at the bottom, offers Jump to latest once
  // they scroll up, and with `history` loads older parts as they near the
  // top, keeping what they were reading in place.

  interface Props {
    /** The agent the conversation belongs to. */
    agent: string;
    items: FeedItem[];
    /** The region's accessible name, such as "Conversation with atlas". */
    label: string;
    history?: FeedHistory;
    /** The items haven't arrived yet: the empty state waits. */
    loading?: boolean;
    /** Something at the tail is growing (a turn in progress). */
    live?: boolean;
    /** The correlation id of the turn in flight, whose block is live. */
    liveTurnId?: string | null;
    /** What the page saw of each turn while it ran: timing, how it ended, missed steps. */
    observed?: ObservedTurnLookup;
    /** Stops the turn in flight, from the head of its block. */
    onStop?: () => void;
    /**
     * What a screen reader is told as the agent works: each a new object, so
     * the same words twice in a row are read twice.
     */
    announcement?: { id: number; text: string } | null;
    /** What shows when there are no items. */
    empty?: Snippet;
    /** Live content after the items, such as the turn in progress. */
    tail?: Snippet;
  }

  let {
    agent,
    items,
    label,
    history,
    loading = false,
    live = false,
    liveTurnId = null,
    observed,
    onStop,
    announcement = null,
    empty,
    tail,
  }: Props = $props();

  // A turn that ended before it did anything still shows how it ended.
  const shown = $derived(
    groupTurns(items, liveTurnId, (turnId) => {
      const ending = observed?.(turnId)?.ending;
      return ending === "stopped" || ending === "interrupted";
    }),
  );
  const isEmpty = $derived(items.length === 0 && !loading && liveTurnId === null);

  let scrollEl = $state<HTMLDivElement>();
  let innerEl = $state<HTMLDivElement>();
  let topSentinel = $state<HTMLDivElement>();

  const uid = $props.id();
  /**
   * Near enough the bottom to keep following. The composer sits under the
   * feed rather than over it, so only a reader who has scrolled away stops.
   */
  const FOLLOW_WITHIN_PX = 120;
  const scroller = new FeedScroller(FOLLOW_WITHIN_PX);
  let anchorLabel = $state("");

  $effect(() => {
    if (!scrollEl || !innerEl) return;
    return scroller.attach(scrollEl, innerEl);
  });

  /** Where `el` sits below the top of the scrolling area. */
  function offsetIn(scroll: HTMLElement, el: HTMLElement): number {
    return el.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
  }

  // The pill names the topmost divider in view, or else the last one above it.
  function updateAnchorLabel(): void {
    const el = scrollEl;
    if (!el || !scroller.scrolledUp) {
      anchorLabel = "";
      return;
    }
    let found = "";
    for (const divider of el.querySelectorAll<HTMLElement>("[data-divider-label]")) {
      const top = offsetIn(el, divider);
      const text = divider.dataset.dividerLabel ?? "";
      if (top >= 0) {
        if (top < el.clientHeight) found = text;
        break;
      }
      found = text;
    }
    anchorLabel = found;
  }

  $effect(() => {
    const el = scrollEl;
    if (!el) return;
    el.addEventListener("scroll", updateAnchorLabel, { passive: true });
    return () => el.removeEventListener("scroll", updateAnchorLabel);
  });

  // ── Older history ──────────────────────────────────────────────────

  /** Within this distance above the view, the top counts as reached. */
  const LOAD_OLDER_MARGIN_PX = 200;

  function sentinelNearView(): boolean {
    if (!scrollEl || !topSentinel) return false;
    const sentinel = topSentinel.getBoundingClientRect();
    const view = scrollEl.getBoundingClientRect();
    return sentinel.bottom >= view.top - LOAD_OLDER_MARGIN_PX && sentinel.top <= view.bottom;
  }

  /**
   * A load is between measuring where the reader is and putting them back.
   * Loads run one at a time: one that measured while another's part was in
   * but not yet compensated for would undo that compensation.
   */
  let prepending = false;

  async function loadOlder(): Promise<void> {
    const el = scrollEl;
    if (!history || !el || !history.hasMore || history.loadingOlder || prepending) return;
    // While a reload is putting the reader back, that owns the scroll position;
    // loading resumes once they're back in place.
    if (reloadAnchor !== null) return;
    prepending = true;
    try {
      // Keep what the reader sees in place across the prepend: remember where
      // the first item sits and put it back there. Browser scroll anchoring
      // is off, and it doesn't act at scrollTop 0 anyway.
      const anchor = el.querySelector<HTMLElement>("[data-feed-item]");
      const before = anchor ? offsetIn(el, anchor) : 0;
      const added = await history.loadOlder();
      if (!added) return;
      await tick();
      if (anchor && !scroller.isFollowing) {
        el.scrollTo({ top: el.scrollTop + offsetIn(el, anchor) - before, behavior: "instant" });
      }
    } finally {
      prepending = false;
    }
    // A feed shorter than the view leaves the sentinel in it, so the observer
    // never fires again: keep loading until it's pushed out or history runs
    // out. A failed load returned above, so an error isn't repeated.
    if (sentinelNearView()) void loadOlder();
  }

  $effect(() => {
    const el = scrollEl;
    const sentinel = topSentinel;
    if (!el || !sentinel || !history) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) void loadOlder();
      },
      { root: el, rootMargin: `${LOAD_OLDER_MARGIN_PX}px 0px 0px 0px`, threshold: 0 },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  });

  // The observer fires when the sentinel first shows, which can be before the
  // history says there's more; nothing would fire again, so loading resumes
  // whenever the history's state changes with the sentinel still near.
  $effect(() => {
    void history?.hasMore;
    void history?.loadingOlder;
    void history?.generation;
    void tick().then(() => {
      if (sentinelNearView()) void loadOlder();
    });
  });

  // ── Following ──────────────────────────────────────────────────────

  let lastTailId: number | undefined;
  let lastLength = 0;
  let lastLive = false;

  // A reader at the bottom stays there as the feed grows, at the tail or by
  // a prepend at the head. Their own message, and the first items, always
  // scroll to the bottom.
  $effect(() => {
    const tailItem = items.at(-1);
    const tailChanged = tailItem?.id !== lastTailId;
    const force =
      (tailChanged && tailItem?.kind === "user" && !tailItem.sender) ||
      (lastTailId === undefined && tailItem !== undefined);
    const changed = tailChanged || items.length !== lastLength || live !== lastLive;
    lastTailId = tailItem?.id;
    lastLength = items.length;
    lastLive = live;
    if (changed) {
      void tick().then(() => {
        scroller.contentChanged(force);
      });
    }
  });

  // ── Reloads ────────────────────────────────────────────────────────

  // A reload replaces every item, so a reader who had scrolled up is anchored
  // by content: the topmost message in view is found again by its kind and
  // text and put back where it was, as soon as it's back in the feed, which
  // may be only once the older part it now belongs to has loaded.
  interface ContentAnchor {
    key: string;
    nth: number;
    offset: number;
  }
  let reloadAnchor: ContentAnchor | null = null;
  let seenGeneration: number | null = null;

  function itemElements(): HTMLElement[] {
    return scrollEl ? Array.from(scrollEl.querySelectorAll<HTMLElement>("[data-feed-item]")) : [];
  }

  function contentKey(el: HTMLElement): string {
    return `${el.dataset.kind ?? ""}\u0000${el.textContent}`;
  }

  function captureAnchor(): ContentAnchor | null {
    const el = scrollEl;
    if (!el) return null;
    const elements = itemElements();
    const topmost = elements.find(
      (item) => item.getBoundingClientRect().bottom > el.getBoundingClientRect().top,
    );
    if (!topmost) return null;
    const key = contentKey(topmost);
    const nth = elements
      .slice(0, elements.indexOf(topmost))
      .filter((item) => contentKey(item) === key).length;
    return { key, nth, offset: offsetIn(el, topmost) };
  }

  /** Put the anchored item back in place; false when it isn't in the feed yet. */
  function restoreAnchor(anchor: ContentAnchor): boolean {
    const el = scrollEl;
    if (!el) return false;
    const match = itemElements().filter((item) => contentKey(item) === anchor.key)[anchor.nth];
    if (!match) return false;
    el.scrollTo({ top: el.scrollTop + offsetIn(el, match) - anchor.offset, behavior: "instant" });
    return true;
  }

  $effect.pre(() => {
    const generation = history?.generation ?? 0;
    untrack(() => {
      if (seenGeneration !== null && generation !== seenGeneration) {
        reloadAnchor = scroller.isFollowing ? null : captureAnchor();
        if (reloadAnchor) scroller.hold();
      }
      seenGeneration = generation;
    });
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
        if (sentinelNearView()) void loadOlder();
      }
    });
  });
</script>

<div class="feed">
  <!-- Outside the scrolling region, which is no live region: a reply's text streams in piece by piece. This says only whole things, once each. -->
  <div role="status">
    {#if announcement}
      {#key announcement.id}
        <VisuallyHidden>{announcement.text}</VisuallyHidden>
      {/key}
    {/if}
  </div>
  {#if scroller.scrolledUp}
    <div class="feed-pill">
      {#if anchorLabel}
        <span class="feed-pill-label" id="{uid}-where">{anchorLabel}</span>
      {/if}
      <button
        type="button"
        class="feed-pill-jump"
        aria-describedby={anchorLabel ? `${uid}-where` : undefined}
        onclick={() => scroller.jumpToLatest()}
      >
        Jump to latest
      </button>
    </div>
  {/if}
  <!-- A tab stop while it scrolls, so the keyboard can scroll it. Not a live region: a reply's text streams in piece by piece, and the status lines in it announce themselves. -->
  <div
    class="feed-scroll"
    role="region"
    aria-label={label}
    bind:this={scrollEl}
    {@attach keyboardScrollable}
  >
    <div class="feed-column" class:centered={items.length === 0} bind:this={innerEl}>
      {#if history}
        <div class="feed-sentinel" bind:this={topSentinel}></div>
        <div class="feed-loading" aria-hidden={!history.loadingOlder}>
          {#if history.loadingOlder}
            <Spinner size={12} />
            Loading earlier messages…
          {/if}
        </div>
      {/if}
      {#if isEmpty}
        {@render empty?.()}
      {/if}
      {#each shown as entry (entry.key)}
        {#if entry.kind === "turn"}
          <FeedTurn
            turn={entry}
            {agent}
            observed={entry.turnId === null ? undefined : observed?.(entry.turnId)}
            onStop={entry.live ? onStop : undefined}
          />
        {:else}
          <div class="feed-item" data-feed-item data-kind={entry.item.kind}>
            <FeedItemView item={entry.item} {agent} />
          </div>
        {/if}
      {/each}
      {@render tail?.()}
    </div>
  </div>
</div>

<style>
  .feed {
    position: relative;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    color: var(--color-text);
    font-family: var(--font-ui);
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-regular);
    line-height: var(--line-height-ui);
  }

  .feed-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    /* loadOlder() alone moves the scroll position across a prepend. */
    overflow-anchor: none;

    &:focus-visible {
      outline-offset: calc(-1 * var(--focus-outline-width));
    }
  }

  .feed-column {
    display: flex;
    flex-direction: column;
    gap: var(--space-18);
    width: 100%;
    max-width: calc(var(--layout-reading-width) + 2 * var(--space-24));
    margin: 0 auto;
    padding: var(--space-8) var(--space-24) var(--space-24);

    &.centered {
      min-height: 100%;
      justify-content: center;
    }
  }

  .feed-sentinel {
    height: 1px;
    margin-bottom: calc(-1 * var(--space-18));
  }

  /* A fixed slot, so the loading line never shifts the feed. */
  .feed-loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-8);
    min-height: var(--space-20);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .feed-item {
    min-width: 0;
  }

  /* Centered over the feed, as wide as its label needs and no wider than the feed. */
  .feed-pill {
    position: absolute;
    top: var(--space-12);
    right: var(--space-16);
    left: var(--space-16);
    z-index: var(--z-sticky);
    display: flex;
    align-items: center;
    gap: var(--space-10);
    width: fit-content;
    margin-inline: auto;
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-12);
    border-radius: var(--corner-pill);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
    font-size: var(--font-size-xs);
  }

  .feed-pill-label {
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-2);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .feed-pill:not(:has(.feed-pill-label)) {
    padding-left: var(--space-2);
  }

  .feed-pill-jump {
    flex: none;
    min-height: 28px;
    padding: 0 var(--space-12);
    border-radius: var(--corner-pill);
    background: var(--color-vein-tint);
    color: var(--color-vein-bright);
    font-weight: var(--font-weight-medium);
    transition: background var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-vein-line);
    }
  }

  @media (max-width: 760px) {
    .feed-column {
      padding: var(--space-8) var(--space-16) var(--space-16);
    }

    .feed-pill-jump {
      min-height: var(--layout-touch-target);
    }
  }
</style>
