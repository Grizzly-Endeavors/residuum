// ── Feed scroll following (Svelte 5 runes) ───────────────────────────
//
// Shared by the main chat and session transcripts. A feed follows new content,
// keeping the reader at the bottom as it lands, until the reader scrolls up.
// From then on new items leave them where they are, and a "Jump to latest" pill
// offers the way back. They follow again once they are back at the end: by
// scrolling there themselves, with End, with the pill, or by sending a message.
//
// Leaving is the reader's doing, and takes effect at once: a wheel or key that
// scrolls the feed up, or the scroll position moving up by any amount. How far
// from the end they are doesn't come into it. Coming back is judged by where
// they are: a move that brings them within `followWithinPx` of the end follows
// again.
//
// The scroller is the only code in the page that moves the feed's scroll
// position, and it records each position it sets. A scroll event that finds
// the position somewhere else was moved by the reader: by hand, by momentum
// after a flick, by the scrollbar, or with no input event in the feed at all
// (find in page, a screen reader, the page's own `scrollIntoView`). Three moves
// are the browser's, not the reader's: its glide for End, which can move up a
// little first as Chromium carries a keyboard scroll still under way into it
// (the glide ends on arriving, at `scrollend`, at reader input or at the
// scroller's next move of its own); moving the position up to keep it in range
// when the content or the view shrinks past it, judged against the content as
// it is and as it was before anything grew in place (`keptInRange`); and a move
// it makes while the page changes what the feed shows (`takeInUpdate`).
//
// "The end" means the end of the content, not the end of the scrolling area.
// A feed can reserve room below its last line (a persistent buffer, and a
// composer floating over the area's foot), and that room never counts against
// the reader.

/** Within this distance of the end of the content, a reader back from further up follows again. */
const FOLLOW_THRESHOLD_PX = 120;

/** Sub-pixel scroll positions leave a reader at the bottom a pixel or so short of it. */
const SCROLL_END_SLACK_PX = 2;

/** Input that means the reader is taking the scroll position over. */
const READER_INPUT_EVENTS = ["wheel", "touchstart", "keydown", "pointerdown"] as const;

/** Keys that scroll a focused scrolling area up, whatever modifier is held with them. */
const UP_KEYS = new Set(["ArrowUp", "PageUp", "Home"]);

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
  /** How near the end of the content a reader coming back down must be to follow again. */
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
  /** New content keeps the reader at the bottom. Only the reader's own doing changes it. */
  private following = true;
  /**
   * Where the scroll position is, as far as the scroller knows: where it last
   * put it, or where the last move it took in left it. `null` until it has
   * seen the feed laid out.
   */
  private knownTop: number | null = null;
  /** How tall the content was where the scroller last took the position in. */
  private knownHeight = 0;
  /**
   * The browser's glide for End is under way: its moves, either way, aren't
   * the reader's.
   */
  private endGlide = false;
  /**
   * The page has said its items are about to change (`takeInPendingScroll`),
   * and the `contentChanged` of that change hasn't come yet. Both come in one
   * task, with no frame between them for the reader to scroll in.
   */
  private updating = false;
  /** The feed was hidden when the scroller last looked: it reports no size, and no position worth judging. */
  private hidden = false;
  /**
   * The feed is being rebuilt under a reader who had scrolled up; the scroll
   * position it passes through meanwhile isn't theirs, so it doesn't count.
   */
  private held = false;
  /** Where the content ended and how tall the area was when last looked at. */
  private lastEnd = 0;
  private lastViewport = 0;
  /** The room reserved under the last line when the scroller last looked. */
  private lastRoom = 0;
  /**
   * How much floated over the foot when the scroller last pinned the reader
   * to the bottom. Room that grows while that is unchanged is the page
   * catching up with the room the pin was meant to include (some browsers
   * restyle the column a frame after the room changes), not the composer
   * growing, so it keeps the reader at the bottom.
   */
  private pinnedCovered: number | null = null;
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

  private readonly onScroll = (): void => {
    this.updating = false;
    this.takeInScroll();
  };

  /**
   * Take in where the scroll position is now. A move the scroller didn't make
   * is the reader's, unless it is the browser's.
   */
  private takeInScroll(): void {
    const el = this.el;
    if (!el) return;
    if (isHidden(el)) {
      this.hidden = true;
      return;
    }
    const top = el.scrollTop;
    const from = this.knownTop;
    const fromHeight = this.knownHeight;
    this.know(top);
    // Where the browser has the position as the feed shows again isn't the reader's move.
    if (this.hidden) {
      this.hidden = false;
      return;
    }
    if (from === null || Math.abs(top - from) < 1) return;
    const up = top < from;
    // The browser's glide for End can move up first: Chromium carries a
    // keyboard scroll still under way (Home, a moment before) into the next one.
    if (this.endGlide) {
      if (this.atBottom()) this.endGlide = false;
      return;
    }
    if (this.held) return;
    if (up && !this.keptInRange(from, top, fromHeight)) {
      this.following = false;
    } else if (this.nearEnd()) {
      this.following = true;
    }
    this.refreshPill();
  }

  /** Record `top` as where the position is, over the content as it is now. */
  private know(top: number): void {
    this.knownTop = top;
    this.knownHeight = this.el?.scrollHeight ?? 0;
  }

  /**
   * Whether a move up from `from` to `top` is the browser keeping the position
   * in range: where it was is past the end of the range, and it lands at the
   * end. The range is judged both as it is now and as it was where the scroller last took the
   * position in (`fromHeight`), since content can grow in place (an image
   * loading, a font swapping in) after the move and before its event: the view
   * grows, the browser keeps the position in range, and then the content below
   * a following reader grows.
   */
  private keptInRange(from: number, top: number, fromHeight: number): boolean {
    const el = this.el;
    if (!el) return false;
    const view = el.clientHeight;
    return [el.scrollHeight, fromHeight].some((height) => {
      const end = height - view;
      return from > end + 0.5 && Math.abs(end - top) <= SCROLL_END_SLACK_PX;
    });
  }

  /**
   * Take in a move of the scroll position whose event hasn't arrived yet. The
   * page calls it just before it changes the feed's items, so that a move is
   * judged against the content it was made over. Seen on a phone as the chat
   * opened: the browser moved the position back 4px to keep it in range as
   * the page settled around the feed, and an older episode was prepended
   * before that move's event arrived. Against the episode, the browser's move
   * looks like the reader leaving the bottom; against the content it was made
   * over, it leaves them at the very bottom, where they were.
   */
  takeInPendingScroll(): void {
    this.takeInScroll();
    this.updating = true;
  }

  /**
   * Where the browser moved the position while the page changed what the feed
   * shows (its items, or the turn in flight) isn't the reader's move. Seen in
   * WebKit on every chat open: an older episode replaced the feed's loading
   * row, and the position went up 36px, as far as the content without either
   * allowed, before the episode was laid out. Against the episode, that move
   * looked like the reader leaving the bottom, and the chat opened 850px short
   * of its newest message. Seen in WebKit too as a sent message's turn
   * started and its block appeared: the position went up 1575px, and the
   * reader was left far above their own message.
   */
  private takeInUpdate(): void {
    const el = this.el;
    this.updating = false;
    if (el && !isHidden(el) && !this.hidden) this.know(el.scrollTop);
  }

  /** Scroll at once to `top`, as the scroller's own move, and record where it left the position. */
  private write(top: number): void {
    const el = this.el;
    if (!el) return;
    el.scrollTo({ top, behavior: "instant" });
    this.know(el.scrollTop);
    this.hidden = false;
    // An instant scroll ends a glide under way.
    this.endGlide = false;
  }

  /**
   * Move the scroll position for the page: keeping what the reader sees in
   * place as older history goes in above it, or putting them back where they
   * were after a reload. It is the scroller's own move, never the reader's.
   */
  moveTo(top: number): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.takeInScroll();
    this.write(top);
  }

  // The reader taking over the scroll ends a glide in progress, a reload's
  // hold on their state, and any hold on the place of a control they pressed.
  private readonly onReaderInput = (event: Event): void => {
    this.endGlide = false;
    this.held = false;
    this.letGoOfPlace();
    if (event.type === "wheel") {
      if (this.wheelScrollsUp(event as WheelEvent)) this.leave();
    } else if (event.type === "keydown") {
      if (isEndKey(event)) this.followFromEnd();
      else if (this.keyScrollsUp(event as KeyboardEvent)) this.leave();
    }
  };

  private wheelScrollsUp(event: WheelEvent): boolean {
    // Ctrl zooms, Shift turns the wheel sideways, and a swipe that is mostly
    // sideways scrolls something sideways (a code block, a table).
    if (!(event.deltaY < 0) || event.ctrlKey || event.shiftKey) return false;
    if (Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return false;
    return this.canScrollUp();
  }

  private keyScrollsUp(event: KeyboardEvent): boolean {
    if (event.defaultPrevented || event.altKey) return false;
    // Space pages, with Shift up, only from the area itself: on a control it presses the control.
    const up =
      UP_KEYS.has(event.key) || (event.key === " " && event.shiftKey && event.target === this.el);
    return up && this.canScrollUp();
  }

  /** Whether the feed has anywhere to scroll up to. */
  private canScrollUp(): boolean {
    return this.el !== null && this.el.scrollTop > 0;
  }

  /** The reader scrolled up: stop following at once, however near the end they still are. */
  private leave(): void {
    this.following = false;
    this.refreshPill();
  }

  /** A glide that runs to its end, or is cut short, is over. */
  private readonly onScrollEnd = (): void => {
    this.endGlide = false;
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
    if (!el) return;
    if (isHidden(el)) {
      this.hidden = true;
      return;
    }
    const shown = this.hidden;
    this.updating = false;
    this.takeInScroll();
    const end = this.contentEnd();
    const viewport = el.clientHeight;
    const room = this.bottomRoom();
    const reshaped = shown || Math.abs(end - this.lastEnd) >= 1 || viewport !== this.lastViewport;
    const roomCaughtUp = room > this.lastRoom && this.covered() === this.pinnedCovered;
    this.lastEnd = end;
    this.lastViewport = viewport;
    this.lastRoom = room;
    this.restorePlace();
    if (this.following) {
      // Content growing, or the area changing size, keeps a following reader
      // at the bottom. Only the reserved room changing (the composer growing
      // or shrinking) leaves the thread where it is, unless the last line
      // would end up under what floats over the foot.
      if (reshaped || roomCaughtUp) this.pinToBottom();
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
   * Attaching reads no reactive state, and leaves the reader's state as it was.
   */
  attach(el: HTMLElement, content: HTMLElement): () => void {
    this.el = el;
    this.content = content;
    const resizes = new ResizeObserver(this.onResize);
    // The border box: the room reserved under the last line is padding.
    resizes.observe(content, { box: "border-box" });
    resizes.observe(el);
    el.addEventListener("scroll", this.onScroll, { passive: true });
    el.addEventListener("scrollend", this.onScrollEnd, { passive: true });
    for (const type of READER_INPUT_EVENTS) {
      el.addEventListener(type, this.onReaderInput, { passive: true });
    }
    // Capture: before the control's own click handler changes what is shown.
    el.addEventListener("click", this.onPress, true);
    this.hidden = isHidden(el);
    this.know(el.scrollTop);
    if (this.hidden) this.knownTop = null;
    this.lastEnd = this.contentEnd();
    this.lastViewport = el.clientHeight;
    this.lastRoom = this.bottomRoom();
    return () => {
      resizes.disconnect();
      el.removeEventListener("scroll", this.onScroll);
      el.removeEventListener("scrollend", this.onScrollEnd);
      for (const type of READER_INPUT_EVENTS) el.removeEventListener(type, this.onReaderInput);
      el.removeEventListener("click", this.onPress, true);
      this.letGoOfPlace();
      this.endGlide = false;
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
    if (this.updating) this.takeInUpdate();
    if (force) {
      this.following = true;
      this.pinToBottom();
      return;
    }
    this.takeInScroll();
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
    this.endGlide = false;
    this.scrolledUp = true;
  }

  /** The rebuild is done; judge where the reader is afresh. */
  release(): void {
    if (!this.held) return;
    this.held = false;
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.know(el.scrollTop);
    this.following = this.nearEnd();
    this.refreshPill();
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
    this.scrolledUp = false;
    this.unseen = false;
    if (prefersReducedMotion()) {
      this.write(el.scrollHeight);
      return;
    }
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }

  /**
   * The reader pressed End: follow the newest content again while the
   * browser's own glide gets there. Content landing on the way (an older
   * episode prepended at the top they just left) keeps them pinned to the
   * bottom instead of holding them where the glide had reached.
   */
  private followFromEnd(): void {
    this.following = true;
    // Already at the bottom, the browser has nowhere to glide.
    this.endGlide = !this.atBottom();
    this.scrolledUp = false;
    this.unseen = false;
  }

  /** Put the reader at the true bottom, the reserved room included. */
  private pinToBottom(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    // Instant, not smooth: a smooth scroll still in flight when more content
    // lands stops short of the new bottom.
    this.write(el.scrollHeight);
    this.pinnedCovered = this.covered();
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

  /** At the very bottom of the scroll range, where there is no further to go. */
  private atBottom(): boolean {
    const el = this.el;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight <= SCROLL_END_SLACK_PX;
  }

  /**
   * Whether the reader is at the end: their last line is near enough the view,
   * or they are at the very bottom of the scroll range. The room under the
   * last line is a CSS variable set from the composer's measured height a
   * frame after the composer changes, so for that frame a reader at the
   * bottom can see the composer cover more than the room left for it.
   */
  private nearEnd(): boolean {
    return this.atBottom() || this.reach() <= this.followWithinPx;
  }

  /** Scroll just far enough that something floating over the foot isn't covering the last line. */
  private keepLastLineClear(): void {
    const el = this.el;
    if (!el) return;
    const reach = this.reach();
    if (reach > 0) this.write(el.scrollTop + reach);
  }

  /** The pill, from whether the reader follows and how far from the end they are. */
  private refreshPill(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.scrolledUp = !this.following && !this.nearEnd();
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
    this.endGlide = false;
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
    if (Math.abs(delta) > 0.5) this.write(el.scrollTop + delta);
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
