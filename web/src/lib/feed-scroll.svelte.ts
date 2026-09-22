// ── Feed scroll following (Svelte 5 runes) ───────────────────────────
//
// Shared by the main chat and session views: a feed follows new content only
// while the reader is at the bottom. Once they scroll up to read, new items
// leave them where they are and a "Jump to latest" pill offers the way back.

/** Within this distance of the bottom, the feed keeps following new content. */
const FOLLOW_THRESHOLD_PX = 120;

/** Input that means the reader is scrolling by hand. */
const READER_SCROLL_EVENTS = ["wheel", "touchstart", "keydown", "pointerdown"] as const;

function isHidden(el: HTMLElement): boolean {
  return el.clientHeight === 0;
}

export class FeedScroller {
  /** The reader has scrolled away from the bottom; show "Jump to latest". */
  scrolledUp = $state(false);

  private el: HTMLElement | null = null;
  // Decided by where the reader last left the scroll position, not by the
  // current distance: new content growing below them must not unstick them.
  private following = true;
  /** A "Jump to latest" glide is under way; its passing scroll events don't unstick. */
  private jumping = false;

  private readonly onScroll = (): void => {
    this.measure();
  };

  // The reader taking over the scroll cancels a glide in progress.
  private readonly onReaderScroll = (): void => {
    this.jumping = false;
  };

  /**
   * Start tracking the scrolling `el`, whose `content` element holds the
   * feed; returns the cleanup. Content growing in place (a tool result
   * filling in, an image loading) keeps a following reader at the bottom.
   */
  attach(el: HTMLElement, content: HTMLElement): () => void {
    this.el = el;
    const resizes = new ResizeObserver(() => {
      if (this.following) this.pinToBottom();
    });
    resizes.observe(content);
    el.addEventListener("scroll", this.onScroll, { passive: true });
    for (const type of READER_SCROLL_EVENTS) {
      el.addEventListener(type, this.onReaderScroll, { passive: true });
    }
    this.measure();
    return () => {
      resizes.disconnect();
      el.removeEventListener("scroll", this.onScroll);
      for (const type of READER_SCROLL_EVENTS) el.removeEventListener(type, this.onReaderScroll);
      if (this.el === el) this.el = null;
    };
  }

  /** Whether new content should keep the feed pinned to the bottom. */
  get isFollowing(): boolean {
    return this.following;
  }

  /**
   * Content changed: stay pinned to the bottom if following (or `force`d,
   * e.g. for the reader's own message or a freshly loaded feed).
   */
  contentChanged(force = false): void {
    if (force) this.following = true;
    if (!this.following) return;
    this.pinToBottom();
  }

  /** The pill's action: glide back to the newest content and follow it again. */
  jumpToLatest(): void {
    const el = this.el;
    if (!el) return;
    this.following = true;
    this.jumping = true;
    this.scrolledUp = false;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }

  private pinToBottom(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    // Instant, not smooth: a smooth scroll still in flight when more content
    // lands stops short of the new bottom.
    el.scrollTo({ top: el.scrollHeight, behavior: "instant" });
    this.scrolledUp = false;
  }

  private measure(): void {
    const el = this.el;
    // A hidden feed (the chat under a session view) reports no size; keep the
    // reader's state for when it's shown again.
    if (!el || isHidden(el)) return;
    const distFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    const nearBottom = distFromBottom <= FOLLOW_THRESHOLD_PX;
    if (this.jumping) {
      if (!nearBottom) return;
      this.jumping = false;
    }
    this.scrolledUp = !nearBottom;
    this.following = nearBottom;
  }
}
