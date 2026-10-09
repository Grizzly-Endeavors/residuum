<script lang="ts">
  import { tick, untrack, type Snippet } from "svelte";
  import { FeedScroller } from "../lib/feed-scroll.svelte";
  import { Icon } from "../lib/icons";
  import type { ObservedTurnLookup } from "../lib/observed-turns.svelte";
  import type { FeedItem } from "../lib/types";
  import { keyboardScrollable, Spinner } from "../lib/ui";
  import type { FeedHistory } from "./feed-history";
  import FeedItemView from "./FeedItemView.svelte";
  import FeedTurn from "./FeedTurn.svelte";
  import { groupTurns } from "./turns";

  // A conversation in the reading column: the main chat, or a session's
  // transcript, with each turn's output grouped (`turns.ts`). It follows new
  // content while the reader is at the bottom, offers Jump to latest once
  // they scroll up (and says when something new has landed below them), and
  // with `history` loads older parts as they near the top, keeping what they
  // were reading in place. Opening or closing something in it leaves the
  // control that was pressed where it was.
  //
  // The scrolling area runs the whole height of the feed, so its scrollbar
  // does too. A `dock`, such as the chat's composer, floats over the foot of
  // it; the column keeps its last lines clear of the dock, and, beside the
  // dock, leaves room below them: about a quarter of the view on a wide
  // screen, so the end of the newest reply sits near eye level while the
  // reader follows it, and a little on a phone.

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
    /** Stops the turn in flight, from its activity line. */
    onStop?: () => void;
    /** What shows when there are no items. */
    empty?: Snippet;
    /** Live content after the items, such as the turn in progress. */
    tail?: Snippet;
    /** Content floating over the foot of the feed, such as a composer; the feed measures it. */
    dock?: Snippet;
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
    empty,
    tail,
    dock,
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
  let dockEl = $state<HTMLDivElement>();

  /** The dock's height now. */
  let dockHeight = $state(0);
  /**
   * The room the column keeps for the dock. It follows the dock up at once
   * and down only when that can't move what the reader is looking at: while
   * they sit at the true bottom, a dock that shrinks (a sent message empties
   * the composer) would otherwise pull the whole thread down after it.
   */
  let reservedHeight = $state(0);
  /** The width the area's scrollbar takes, which the dock and the pill stay clear of. */
  let scrollbarWidth = $state(0);

  const scroller = new FeedScroller({ covered: () => dockHeight });

  $effect(() => {
    if (!scrollEl || !innerEl) return;
    return scroller.attach(scrollEl, innerEl);
  });

  $effect(() => {
    const area = scrollEl;
    const floating = dockEl;
    if (!area) return;
    const measure = (): void => {
      scrollbarWidth = area.offsetWidth - area.clientWidth;
      dockHeight = floating?.offsetHeight ?? 0;
      reservedHeight = Math.max(reservedHeight, dockHeight);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(area);
    if (floating) observer.observe(floating);
    untrack(measure);
    return () => {
      observer.disconnect();
      if (!floating) return;
      dockHeight = 0;
      reservedHeight = 0;
    };
  });

  // Room kept for a dock that has shrunk is given back once the reader has
  // moved far enough from the bottom for that to go unseen, and with new content.
  function releaseSpareRoom(): void {
    const area = scrollEl;
    if (!area || reservedHeight === dockHeight) return;
    const spare = reservedHeight - dockHeight;
    if (area.scrollHeight - area.scrollTop - area.clientHeight >= spare) {
      reservedHeight = dockHeight;
    }
  }

  $effect(() => {
    const area = scrollEl;
    if (!area) return;
    area.addEventListener("scroll", releaseSpareRoom, { passive: true });
    return () => area.removeEventListener("scroll", releaseSpareRoom);
  });

  /** Where `el` sits below the top of the scrolling area. */
  function offsetIn(scroll: HTMLElement, el: HTMLElement): number {
    return el.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
  }

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
  let lastGeneration = 0;

  // A reader at the bottom stays there as the feed grows, at the tail or by
  // a prepend at the head. Their own message, and the first items, always
  // scroll to the bottom. A new item at the tail of a feed whose reader is
  // elsewhere is told to the pill.
  $effect(() => {
    const tailItem = items.at(-1);
    const generation = history?.generation ?? 0;
    const tailChanged = tailItem?.id !== lastTailId;
    const force =
      (tailChanged && tailItem?.kind === "user" && !tailItem.sender) ||
      (lastTailId === undefined && tailItem !== undefined);
    // A reload replaces every item, so its new tail isn't something that arrived.
    const arrived =
      tailChanged &&
      tailItem !== undefined &&
      lastTailId !== undefined &&
      generation === lastGeneration;
    const changed = tailChanged || items.length !== lastLength || live !== lastLive;
    lastTailId = tailItem?.id;
    lastLength = items.length;
    lastLive = live;
    lastGeneration = generation;
    if (changed) {
      void tick().then(() => {
        reservedHeight = dockHeight;
        scroller.contentChanged(force, arrived);
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

<div
  class="feed"
  data-docked={dock !== undefined || undefined}
  style:--feed-dock-height="{dockHeight}px"
  style:--feed-reserved="{reservedHeight}px"
  style:--feed-scrollbar="{scrollbarWidth}px"
>
  {#if scroller.scrolledUp}
    <!-- A status region, so the pill turning into "New reply" is announced. -->
    <div class="feed-pill" role="status">
      <button
        type="button"
        class="feed-pill-jump"
        data-new={scroller.unseen || undefined}
        aria-label={scroller.unseen ? "New reply, jump to latest" : undefined}
        onclick={() => scroller.jumpToLatest()}
      >
        {#if scroller.unseen}
          <span class="feed-pill-dot" aria-hidden="true"></span>
        {/if}
        <Icon name="arrow-down" size={14} />
        {scroller.unseen ? "New reply" : "Jump to latest"}
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
  {#if dock}
    <div class="feed-dock" bind:this={dockEl}>
      {@render dock()}
    </div>
  {/if}
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

  /* The column sizes the room under its last line by the view's height. */
  .feed[data-docked] .feed-scroll {
    container-type: size;
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

  /* Under a dock the column keeps the dock's room, and a little more. */
  .feed[data-docked] .feed-column {
    padding-bottom: calc(var(--feed-reserved) + var(--space-48));
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

  /* Floats over the foot of the feed, beside the scrollbar, and takes up the dock's height. */
  .feed-dock {
    position: absolute;
    right: var(--feed-scrollbar);
    bottom: 0;
    left: 0;
    z-index: var(--z-sticky);
    background: var(--color-stone-0);

    /* The lines scrolling under the dock fade out above it, so they don't stop at its edge. */
    &::before {
      content: "";
      position: absolute;
      right: 0;
      bottom: 100%;
      left: 0;
      height: var(--space-24);
      background: linear-gradient(to top, var(--color-stone-0), transparent);
      pointer-events: none;
    }
  }

  /* Centered above the dock, as wide as its label needs. */
  .feed-pill {
    position: absolute;
    right: var(--feed-scrollbar);
    bottom: calc(var(--feed-dock-height) + var(--space-12));
    left: 0;
    z-index: var(--z-sticky);
    display: flex;
    justify-content: center;
    pointer-events: none;
  }

  .feed-pill-jump {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    min-height: 32px;
    padding: 0 var(--space-14) 0 var(--space-12);
    border-radius: var(--corner-pill);
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    pointer-events: auto;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-4);
      color: var(--color-text);
    }

    &[data-new] {
      color: var(--color-text);

      & :global(svg) {
        color: var(--color-vein-bright);
      }
    }
  }

  /* Something landed below: a vein mark before the arrow. */
  .feed-pill-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--color-vein-bright);
  }

  /* On a wide screen the end of the newest reply rests about a quarter of the view above the foot. */
  @media (min-width: 761px) {
    .feed[data-docked] .feed-column:not(.centered) {
      padding-bottom: calc(var(--feed-reserved) + var(--layout-feed-buffer));
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
