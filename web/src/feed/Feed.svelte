<script lang="ts">
  import { flushSync, tick, untrack, type Snippet } from "svelte";
  import { FeedScroller } from "../lib/feed-scroll.svelte";
  import { Icon } from "../lib/icons";
  import type { ObservedTurnLookup } from "../lib/observed-turns.svelte";
  import type { FeedItem } from "../lib/types";
  import { keyboardScrollable, Spinner, VisuallyHidden } from "../lib/ui";
  import type { FeedHistory } from "./feed-history";
  import FeedItemView from "./FeedItemView.svelte";
  import FeedTurn from "./FeedTurn.svelte";
  import { groupTurns } from "./turns";

  interface Props {
    agent: string;
    items: FeedItem[];
    label: string;
    history?: FeedHistory;
    loading?: boolean;
    live?: boolean;
    liveTurnId?: string | null;
    observed?: ObservedTurnLookup;
    onStop?: () => void;
    announcement?: { id: number; text: string } | null;
    empty?: Snippet;
    tail?: Snippet;
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
    announcement = null,
    empty,
    tail,
    dock,
  }: Props = $props();

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

  let dockHeight = $state(0);
  let reservedHeight = $state(0);
  let scrollbarWidth = $state(0);

  const scroller = new FeedScroller({ covered: () => dockHeight });

  $effect(() => {
    const area = scrollEl;
    const column = innerEl;
    if (!area || !column) return;
    return untrack(() => scroller.attach(area, column));
  });

  $effect(() => {
    const area = scrollEl;
    const floating = dockEl;
    if (!area) return;
    const measure = (): void => {
      scrollbarWidth = area.offsetWidth - area.clientWidth;
      dockHeight = floating?.offsetHeight ?? 0;
      const appeared = reservedHeight === 0 && dockHeight > 0;
      reservedHeight = Math.max(reservedHeight, dockHeight);
      if (appeared) {
        flushSync();
        scroller.contentChanged();
      }
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

  function offsetIn(scroll: HTMLElement, el: HTMLElement): number {
    return el.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
  }

  const LOAD_OLDER_MARGIN_PX = 200;

  function sentinelNearView(): boolean {
    if (!scrollEl || !topSentinel) return false;
    const sentinel = topSentinel.getBoundingClientRect();
    const view = scrollEl.getBoundingClientRect();
    return sentinel.bottom >= view.top - LOAD_OLDER_MARGIN_PX && sentinel.top <= view.bottom;
  }

  let prepending = false;

  async function loadOlder(): Promise<void> {
    const el = scrollEl;
    if (!history || !el || !history.hasMore || history.loadingOlder || prepending) return;
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
        scroller.moveTo(el.scrollTop + offsetIn(el, anchor) - before);
      }
    } finally {
      prepending = false;
    }
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

  $effect(() => {
    void history?.hasMore;
    void history?.loadingOlder;
    void history?.generation;
    void tick().then(() => {
      if (sentinelNearView()) void loadOlder();
    });
  });

  $effect.pre(() => {
    void items.length;
    void items.at(-1)?.id;
    void live;
    void liveTurnId;
    untrack(() => {
      scroller.takeInPendingScroll();
    });
  });

  let lastTailId: number | undefined;
  let lastLength = 0;
  let lastLive = false;
  let lastLiveTurnId: string | null = null;
  let lastGeneration = 0;

  $effect(() => {
    const tailItem = items.at(-1);
    const generation = history?.generation ?? 0;
    const tailChanged = tailItem?.id !== lastTailId;
    const force =
      (tailChanged && tailItem?.kind === "user" && !tailItem.sender) ||
      (lastTailId === undefined && tailItem !== undefined);
    const arrived =
      tailChanged &&
      tailItem !== undefined &&
      lastTailId !== undefined &&
      generation === lastGeneration;
    const changed =
      tailChanged ||
      items.length !== lastLength ||
      live !== lastLive ||
      liveTurnId !== lastLiveTurnId;
    lastTailId = tailItem?.id;
    lastLength = items.length;
    lastLive = live;
    lastLiveTurnId = liveTurnId;
    lastGeneration = generation;
    if (changed) {
      void tick().then(() => {
        reservedHeight = dockHeight;
        flushSync();
        scroller.contentChanged(force, arrived);
      });
    }
  });

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
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) =>
        node.parentElement?.closest("[data-message-meta]") instanceof Element
          ? NodeFilter.FILTER_REJECT
          : NodeFilter.FILTER_ACCEPT,
    });
    let text = "";
    for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
      text += node.textContent ?? "";
    }
    return `${el.dataset.kind ?? ""}\u0000${text}`;
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

  function restoreAnchor(anchor: ContentAnchor): boolean {
    const el = scrollEl;
    if (!el) return false;
    const match = itemElements().filter((item) => contentKey(item) === anchor.key)[anchor.nth];
    if (!match) return false;
    scroller.moveTo(el.scrollTop + offsetIn(el, match) - anchor.offset);
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
  <!-- Outside the scrolling region, which is no live region: a reply's text streams in piece by piece. This says only whole things, once each. -->
  <div role="status">
    {#if announcement}
      {#key announcement.id}
        <VisuallyHidden>{announcement.text}</VisuallyHidden>
      {/key}
    {/if}
  </div>
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
    --leading: var(--line-height-ui);
  }

  .feed-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    container: feed / inline-size;
    overflow-anchor: none;

    &:focus-visible {
      outline-offset: calc(-1 * var(--focus-outline-width));
    }
  }

  .feed[data-docked] .feed-scroll {
    container-type: size;
  }

  /* Positioned, so absolutely placed text for screen readers in the messages
     stays inside the scrolling area instead of making the whole page scroll. */
  .feed-column {
    position: relative;
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

  .feed[data-docked] .feed-column {
    padding-bottom: calc(var(--feed-reserved) + var(--space-48));
  }

  .feed-sentinel {
    height: 1px;
    margin-bottom: calc(-1 * var(--space-18));
  }

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

  .feed-dock {
    position: absolute;
    right: var(--feed-scrollbar);
    bottom: 0;
    left: 0;
    z-index: var(--z-sticky);
    background: var(--feed-surface, var(--color-stone-0));

    &::before {
      content: "";
      position: absolute;
      right: 0;
      bottom: 100%;
      left: 0;
      height: var(--space-24);
      background: linear-gradient(to top, var(--feed-surface, var(--color-stone-0)), transparent);
      pointer-events: none;
    }
  }

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

  .feed-pill-dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--color-vein-bright);
  }

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
