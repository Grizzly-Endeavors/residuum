// ── Feed scroll following (Svelte 5 runes) ───────────────────────────
//
// Shared by the main chat and session transcripts: a feed follows new content only
// while the reader is at the bottom. Once they scroll up to read, new items
// leave them where they are and a "Jump to latest" pill offers the way back.
//
// "The bottom" means the end of the content, not the end of the scrolling
// area. A feed can reserve room below its last line (a persistent buffer, and
// a composer floating over the area's foot), and that room never counts
// against the reader: they follow while the last line is within
// `followWithinPx` of the visible part of the area.

/** Within this distance of the end of the content, a feed keeps following new content. */
const FOLLOW_THRESHOLD_PX = 120;

/** Sub-pixel scroll positions leave a reader at the bottom a pixel or so short of it. */
const SCROLL_END_SLACK_PX = 2;

/** Input that means the reader is scrolling by hand. */
const READER_SCROLL_EVENTS = ["wheel", "touchstart", "keydown", "pointerdown"] as const;

/** How long a pressed expand or collapse control keeps its place against layout moving around it. */
const KEEP_PLACE_MS = 500;

/** End with no modifier: the reader asking for the newest content, as the pill does. */
function isEndKey(event: Event): boolean {
  if (event.type !== "keydown") return false;
  const { key, altKey, ctrlKey, metaKey, shiftKey } = event as KeyboardEvent;
  return key === "End" && !altKey && !ctrlKey && !metaKey && !shiftKey;
}

function isHidden(el: HTMLElement): boolean {
  return el.clientHeight === 0;
}

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** Where `el` sits below the top of the scrolling area `scroll`. */
function offsetIn(scroll: HTMLElement, el: HTMLElement): number {
  return el.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
}

/**
 * Where the layout puts `el` below the top of `scroll`, without the nudge a
 * press gives a button (`:active` moves it down a pixel while it is held,
 * and a touch holds it until after the click).
 */
function placeIn(scroll: HTMLElement, el: HTMLElement): number {
  if (typeof DOMMatrix === "undefined") return offsetIn(scroll, el);
  const { transform } = getComputedStyle(el);
  const nudge = transform === "none" || transform === "" ? 0 : new DOMMatrix(transform).m42;
  return offsetIn(scroll, el) - nudge;
}

/** The control the reader pressed to open or close something, and where it was. */
interface PlaceToKeep {
  /** The control, or the one that replaced it: what it opens may redraw it as it goes. */
  control: HTMLElement;
  /** What it controls (`aria-controls`) and its number among its item's such controls, to find it again. */
  controls: string | null;
  position: number;
  /** Its feed item, held in place while the control is away. */
  item: HTMLElement | null;
  controlTop: number;
  itemTop: number;
}

/** Controls that open or close something. */
const EXPANDERS = "[aria-expanded]";

export interface FeedScrollerOptions {
  /** How near the end of the content the visible bottom may be and still count as following. */
  followWithinPx?: number;
  /**
   * Pixels at the foot of the scrolling area that something floating over it
   * hides (a composer): the last line is only in view above them.
   */
  covered?: () => number;
}

export class FeedScroller {
  /** The reader has scrolled away from the end; show "Jump to latest". */
  scrolledUp = $state(false);
  /** Something arrived below the reader while they were scrolled away. */
  unseen = $state(false);

  private el: HTMLElement | null = null;
  private content: HTMLElement | null = null;
  // Decided by where the reader last left the scroll position, not by the
  // current distance: new content growing below them must not unstick them.
  private following = true;
  /** A "Jump to latest" glide is under way; its passing scroll events don't unstick. */
  private jumping = false;
  /**
   * The feed is being rebuilt under a reader who had scrolled up; the scroll
   * position it passes through meanwhile isn't theirs, so it doesn't count.
   */
  private held = false;
  /** Where the content ended and how tall the area was when last looked at. */
  private lastEnd = 0;
  private lastViewport = 0;
  /** An expand or collapse control to keep where it was, for a moment. */
  private keeping: PlaceToKeep | null = null;
  private keepingTimer: ReturnType<typeof setTimeout> | undefined;
  private keepingFrame = 0;
  /** The content is held at its height from the moment of a press, until the reader scrolls. */
  private holdingHeight = false;

  private readonly followWithinPx: number;
  private readonly covered: () => number;

  constructor(options: FeedScrollerOptions = {}) {
    this.followWithinPx = options.followWithinPx ?? FOLLOW_THRESHOLD_PX;
    this.covered = options.covered ?? (() => 0);
  }

  /**
   * The scroll position the scroller last saw or set. A scroll event that
   * finds the position where it was is not the reader moving: it is the late
   * event of the scroller's own instant scroll, which can arrive after more
   * content has landed below and must not count as the reader leaving the end.
   */
  private seenTop: number | null = null;

  private readonly onScroll = (): void => {
    const top = this.el?.scrollTop;
    if (top !== undefined && top === this.seenTop) return;
    this.seenTop = top ?? null;
    this.measure();
  };

  /** Scroll at once to `top`, as the scroller's own move rather than the reader's. */
  private scrollInstantly(top: number): void {
    const el = this.el;
    if (!el) return;
    el.scrollTo({ top, behavior: "instant" });
    this.seenTop = el.scrollTop;
  }

  // The reader taking over the scroll cancels a glide in progress, and ends
  // any hold on the place of a control they pressed.
  private readonly onReaderScroll = (event: Event): void => {
    this.jumping = false;
    this.held = false;
    this.letGoOfPlace();
    if (isEndKey(event)) this.followFromEnd();
  };

  // Seen before the control's own handler: open or close, the control stays
  // where the reader pressed it.
  private readonly onPress = (event: Event): void => {
    const el = this.el;
    if (!el || !(event.target instanceof Element)) return;
    const control = event.target.closest<HTMLElement>(EXPANDERS);
    if (control !== null && el.contains(control)) this.keepPlaceOf(control);
  };

  private readonly onResize = (): void => {
    const el = this.el;
    if (!el || isHidden(el)) return;
    const end = this.contentEnd();
    const viewport = el.clientHeight;
    const reshaped = Math.abs(end - this.lastEnd) >= 1 || viewport !== this.lastViewport;
    this.lastEnd = end;
    this.lastViewport = viewport;
    this.restorePlace();
    if (this.following) {
      // Content growing, or the area changing size, keeps a following reader
      // at the bottom. Only the reserved room changing (the composer growing
      // or shrinking) leaves the thread where it is, unless the last line
      // would end up under what floats over the foot.
      if (reshaped) this.pinToBottom();
      else this.keepLastLineClear();
    } else if (!this.held) {
      this.refreshPill();
    }
  };

  /**
   * Start tracking the scrolling `el`, whose `content` element holds the
   * feed; returns the cleanup. Content growing in place (a tool result
   * filling in, an image loading) keeps a following reader at the bottom, and
   * so does `el` itself changing size under them (the window narrowing).
   */
  attach(el: HTMLElement, content: HTMLElement): () => void {
    this.el = el;
    this.content = content;
    const resizes = new ResizeObserver(this.onResize);
    resizes.observe(content);
    resizes.observe(el);
    el.addEventListener("scroll", this.onScroll, { passive: true });
    for (const type of READER_SCROLL_EVENTS) {
      el.addEventListener(type, this.onReaderScroll, { passive: true });
    }
    // Capture: before the control's own click handler changes what is shown.
    el.addEventListener("click", this.onPress, true);
    this.lastEnd = this.contentEnd();
    this.lastViewport = el.clientHeight;
    this.measure();
    return () => {
      resizes.disconnect();
      el.removeEventListener("scroll", this.onScroll);
      for (const type of READER_SCROLL_EVENTS) el.removeEventListener(type, this.onReaderScroll);
      el.removeEventListener("click", this.onPress, true);
      this.letGoOfPlace();
      if (this.el === el) {
        this.el = null;
        this.content = null;
      }
    };
  }

  /** Whether new content should keep the feed pinned to the bottom. */
  get isFollowing(): boolean {
    return this.following;
  }

  /**
   * Content changed: stay pinned to the bottom if following (or `force`d,
   * e.g. for the reader's own message or a freshly loaded feed). `arrived`
   * says a new item landed at the end, which a reader who isn't following is
   * told about (`unseen`).
   */
  contentChanged(force = false, arrived = false): void {
    if (force) this.following = true;
    if (this.following) {
      this.pinToBottom();
      return;
    }
    if (this.held) return;
    this.refreshPill();
    if (arrived && this.scrolledUp) this.unseen = true;
  }

  /**
   * Keep the reader counted as scrolled up while the feed is rebuilt under
   * them, until `release()` or they scroll by hand.
   */
  hold(): void {
    this.held = true;
    this.following = false;
    this.scrolledUp = true;
  }

  /** The rebuild is done; judge the scroll position afresh. */
  release(): void {
    if (!this.held) return;
    this.held = false;
    this.measure();
  }

  /** Whether a `hold()` is still in effect. */
  get isHeld(): boolean {
    return this.held;
  }

  /** The pill's action: glide back to the newest content and follow it again. */
  jumpToLatest(): void {
    const el = this.el;
    if (!el) return;
    this.letGoOfPlace();
    this.following = true;
    this.held = false;
    this.jumping = true;
    this.scrolledUp = false;
    this.unseen = false;
    el.scrollTo({ top: el.scrollHeight, behavior: prefersReducedMotion() ? "instant" : "smooth" });
  }

  /**
   * The reader pressed End: follow the newest content again while the
   * browser's own glide gets there. Content landing on the way (an older
   * episode prepended at the top they just left) keeps them pinned to the
   * bottom instead of holding them where the glide had reached.
   */
  private followFromEnd(): void {
    this.following = true;
    this.jumping = true;
    this.scrolledUp = false;
    this.unseen = false;
  }

  private pinToBottom(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    // Instant, not smooth: a smooth scroll still in flight when more content
    // lands stops short of the new bottom.
    this.scrollInstantly(el.scrollHeight);
    this.scrolledUp = false;
    this.unseen = false;
  }

  /** The room under the content's last line, which reserves space and is never counted as content. */
  private bottomRoom(): number {
    const content = this.content;
    if (!content) return 0;
    return Number.parseFloat(getComputedStyle(content).paddingBottom) || 0;
  }

  /** Where the last line ends, in the area's scrolled coordinates. */
  private contentEnd(): number {
    const el = this.el;
    return el === null ? 0 : el.scrollHeight - this.bottomRoom();
  }

  /**
   * How far the last line is below the part of the area the reader can see:
   * positive when it is out of view, negative while it is in view with room
   * to spare.
   */
  private reach(): number {
    const el = this.el;
    if (!el) return 0;
    return this.contentEnd() - (el.scrollTop + el.clientHeight - this.covered());
  }

  /**
   * Whether the reader is at the end: their last line is near enough the view,
   * or they are at the very bottom of the scroll range, where there is no
   * further to go. The room under the last line is a CSS variable set from
   * the composer's measured height a frame after the composer changes, so for
   * that frame a reader at the bottom can see the composer cover more than the
   * room left for it.
   */
  private nearEnd(): boolean {
    const el = this.el;
    if (!el) return true;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= SCROLL_END_SLACK_PX;
    return atBottom || this.reach() <= this.followWithinPx;
  }

  /** Scroll just far enough that something floating over the foot isn't covering the last line. */
  private keepLastLineClear(): void {
    const el = this.el;
    if (!el) return;
    const reach = this.reach();
    if (reach > 0) this.scrollInstantly(el.scrollTop + reach);
  }

  /** The reader's position, from their own scrolling. */
  private measure(): void {
    const el = this.el;
    // A hidden feed reports no size; keep the
    // reader's state for when it's shown again.
    if (!el || isHidden(el) || this.held) return;
    const nearEnd = this.nearEnd();
    if (this.jumping) {
      if (!nearEnd) return;
      this.jumping = false;
    }
    this.scrolledUp = !nearEnd;
    this.following = nearEnd;
    if (nearEnd) this.unseen = false;
  }

  /** The pill for a reader who isn't following, as the content around them changes. */
  private refreshPill(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.scrolledUp = !this.nearEnd();
    if (!this.scrolledUp) this.unseen = false;
  }

  // ── Opening and closing things ─────────────────────────────────────

  /**
   * The reader pressed a control that opens or closes something (anything
   * with `aria-expanded`). What it reveals or hides must not carry the
   * control away: it keeps its place on the screen, the feed stops following
   * until the reader is back at the end, and the content's height is held,
   * so closing something at the very end doesn't pull everything down after
   * it. Scrolling by hand lets the height go.
   */
  private keepPlaceOf(control: HTMLElement): void {
    const el = this.el;
    const content = this.content;
    if (!el || !content) return;
    this.following = false;
    this.jumping = false;
    const item = control.closest<HTMLElement>("[data-feed-item]");
    this.keeping = {
      control,
      controls: control.getAttribute("aria-controls"),
      position: [...(item ?? el).querySelectorAll(EXPANDERS)].indexOf(control),
      item,
      controlTop: placeIn(el, control),
      itemTop: item === null ? 0 : offsetIn(el, item),
    };
    // As tall as it is now, whatever an earlier press held it at.
    content.style.minHeight = `${String(content.offsetHeight)}px`;
    this.holdingHeight = true;
    clearTimeout(this.keepingTimer);
    this.keepingTimer = setTimeout(() => {
      this.keeping = null;
    }, KEEP_PLACE_MS);
    // Every frame while it holds: the control can move inside content whose
    // overall size doesn't change, which no resize would report.
    cancelAnimationFrame(this.keepingFrame);
    this.keepingFrame = requestAnimationFrame(this.keepPlaceEachFrame);
  }

  private readonly keepPlaceEachFrame = (): void => {
    if (this.keeping === null) return;
    this.restorePlace();
    this.keepingFrame = requestAnimationFrame(this.keepPlaceEachFrame);
  };

  /** The control the reader pressed, or what stands in for it once what it opened has redrawn it. */
  private controlNow(keeping: PlaceToKeep): HTMLElement | null {
    if (keeping.control.isConnected) return keeping.control;
    const scope = keeping.item?.isConnected ? keeping.item : this.el;
    if (!scope) return null;
    const candidates = [...scope.querySelectorAll<HTMLElement>(EXPANDERS)];
    const controlling =
      keeping.controls === null
        ? undefined
        : candidates.find((other) => other.getAttribute("aria-controls") === keeping.controls);
    const found = controlling ?? candidates[keeping.position];
    if (found) keeping.control = found;
    return found ?? null;
  }

  private restorePlace(): void {
    const el = this.el;
    const keeping = this.keeping;
    if (!el || keeping === null) return;
    const control = this.controlNow(keeping);
    const target = control ?? keeping.item;
    if (!target?.isConnected) return;
    const delta = placeIn(el, target) - (control === null ? keeping.itemTop : keeping.controlTop);
    if (Math.abs(delta) > 0.5) this.scrollInstantly(el.scrollTop + delta);
  }

  private letGoOfPlace(): void {
    clearTimeout(this.keepingTimer);
    this.keepingTimer = undefined;
    if (this.keeping !== null) cancelAnimationFrame(this.keepingFrame);
    this.keeping = null;
    if (this.holdingHeight && this.content) this.content.style.minHeight = "";
    this.holdingHeight = false;
  }
}
