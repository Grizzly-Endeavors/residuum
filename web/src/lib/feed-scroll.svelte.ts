// ── Feed scroll following (Svelte 5 runes) ───────────────────────────
//
// Shared by the main chat and session transcripts. A feed follows new content,
// keeping the reader at the bottom as it lands, until the reader scrolls up.
// From then on new items leave them where they are, and a "Jump to latest" pill
// offers the way back. They follow again once they are back at the end: by
// scrolling there themselves, with End, with the pill, or by sending a message.
//
// The feed takes Home and End over from the browser: Home glides to the top
// and End to the newest content, the way the pill does. WebKit's own animated
// scroll for these keys stops short under load, which would leave End's reader
// partway up the conversation while the feed counts them as following.
//
// The scroller makes its glides itself, one position a frame, rather than with
// the browser's smooth scroll. Script can't see where a smooth scroll is going
// or reliably stop it: in Chromium, a smooth scroll keeps moving after an
// instant scroll to where it is or elsewhere, and a smooth scroll started while
// another runs carries the first one's speed. Each frame of a glide is a
// position the scroller sets and records, a new glide or any other move of its
// own ends the one under way, and End's glide aims at the end as it is on each
// frame. A glide to the end that the reader stops short leaves them where it
// stopped, no longer following.
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
// (find in page, a screen reader, the page's own `scrollIntoView`). Two moves
// are the browser's, not the reader's: moving the position up to keep it in
// range when the content or the view shrinks past it, judged against the
// content as it is and as it was before anything grew in place
// (`keptInRange`); and a move it makes while the page changes what the feed
// shows (`takeInUpdate`).
//
// "The end" means the end of the content, not the end of the scrolling area.
// A feed can reserve room below its last line (a persistent buffer, and a
// composer floating over the area's foot), and that room never counts against
// the reader.

const FOLLOW_THRESHOLD_PX = 120;

const SCROLL_END_SLACK_PX = 2;

const READER_INPUT_EVENTS = ["wheel", "touchstart", "pointerdown"] as const;

const UP_KEYS = new Set(["ArrowUp", "PageUp", "Home"]);

const GLIDE_MIN_MS = 150;
const GLIDE_MS_PER_ROOT_PX = 8;
const GLIDE_MAX_MS = 600;

function glideDuration(distance: number): number {
  return Math.min(GLIDE_MAX_MS, GLIDE_MIN_MS + Math.sqrt(distance) * GLIDE_MS_PER_ROOT_PX);
}

const FRAME_MS = 1000 / 60;

function easeOut(progress: number): number {
  return 1 - (1 - progress) ** 3;
}

const KEEP_PLACE_MS = 500;

const OWNS_HOME_END = [
  "input",
  "textarea",
  "select",
  "audio",
  "video",
  '[role="combobox"]',
  '[role="grid"]',
  '[role="listbox"]',
  '[role="menu"]',
  '[role="menubar"]',
  '[role="menuitem"]',
  '[role="option"]',
  '[role="radiogroup"]',
  '[role="slider"]',
  '[role="spinbutton"]',
  '[role="tab"]',
  '[role="tablist"]',
  '[role="tree"]',
  '[role="treegrid"]',
  '[role="toolbar"]',
].join(", ");

function keyReachesFeed(target: EventTarget | null, feed: HTMLElement): boolean {
  if (target === feed || !(target instanceof Element)) return true;
  if (ownsHomeEnd(target, feed)) return false;
  if (target instanceof HTMLElement && target.isContentEditable) return false;
  return !scrollsInside(target, feed);
}

function ownsHomeEnd(target: Element, feed: HTMLElement): boolean {
  for (let el: Element | null = target; el !== null && el !== feed; el = el.parentElement) {
    if (el.matches(OWNS_HOME_END)) return true;
  }
  return false;
}

function feedKey(event: KeyboardEvent, feed: HTMLElement): "Home" | "End" | null {
  const { key, altKey, ctrlKey, metaKey, shiftKey } = event;
  if (key !== "Home" && key !== "End") return null;
  if (event.defaultPrevented || altKey || ctrlKey || metaKey || shiftKey) return null;
  return keyReachesFeed(event.target, feed) ? key : null;
}

function scrollsInside(target: Element, feed: HTMLElement): boolean {
  for (let el: Element | null = target; el !== null && el !== feed; el = el.parentElement) {
    if (el.scrollHeight <= el.clientHeight) continue;
    const { overflowY } = getComputedStyle(el);
    if (overflowY === "auto" || overflowY === "scroll") return true;
  }
  return false;
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

function offsetIn(scroll: HTMLElement, el: HTMLElement): number {
  return el.getBoundingClientRect().top - scroll.getBoundingClientRect().top;
}

function placeIn(scroll: HTMLElement, el: HTMLElement): number {
  if (typeof DOMMatrix === "undefined") return offsetIn(scroll, el);
  const { transform } = getComputedStyle(el);
  const nudge = transform === "none" || transform === "" ? 0 : new DOMMatrix(transform).m42;
  return offsetIn(scroll, el) - nudge;
}

interface PlaceToKeep {
  control: HTMLElement;
  controls: string | null;
  position: number;
  item: HTMLElement | null;
  controlTop: number;
  itemTop: number;
}

const EXPANDERS = "[aria-expanded]";

export interface FeedScrollerOptions {
  followWithinPx?: number;
  covered?: () => number;
}

export class FeedScroller {
  scrolledUp = $state(false);
  unseen = $state(false);

  private el: HTMLElement | null = null;
  private content: HTMLElement | null = null;
  private following = true;
  private knownTop: number | null = null;
  private knownHeight = 0;
  private updating = false;
  private hidden = false;
  private held = false;
  private lastEnd = 0;
  private lastViewport = 0;
  private lastRoom = 0;
  private pinnedCovered: number | null = null;
  private keeping: PlaceToKeep | null = null;
  private keepingTimer: ReturnType<typeof setTimeout> | undefined;
  private keepingFrame = 0;
  private holdingHeight = false;
  private glideFrame = 0;

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
    if (this.hidden) {
      this.hidden = false;
      return;
    }
    if (from === null || Math.abs(top - from) < 1) return;
    this.interruptGlide();
    const up = top < from;
    if (this.held) return;
    if (up && !this.keptInRange(from, top, fromHeight)) {
      this.following = false;
    } else if (this.nearEnd()) {
      this.following = true;
    }
    this.refreshPill();
  }

  private know(top: number): void {
    this.knownTop = top;
    this.knownHeight = this.el?.scrollHeight ?? 0;
  }

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

  private write(top: number): void {
    this.stopGlide();
    this.place(top);
  }

  private place(top: number): void {
    const el = this.el;
    if (!el) return;
    el.scrollTo({ top, behavior: "instant" });
    this.know(el.scrollTop);
    this.hidden = false;
  }

  moveTo(top: number): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.takeInScroll();
    this.write(top);
  }

  private readonly onReaderInput = (event: Event): void => {
    this.held = false;
    this.letGoOfPlace();
    if (event.type === "keydown") return;
    this.interruptGlide();
    if (event.type === "wheel" && this.wheelScrollsUp(event as WheelEvent)) this.leave();
  };

  private readonly onKeyDown = (event: KeyboardEvent): void => {
    const el = this.el;
    if (!el) return;
    this.onReaderInput(event);
    const key = feedKey(event, el);
    if (key === null) {
      if (this.keyScrollsUp(event)) {
        this.interruptGlide();
        this.leave();
      }
      return;
    }
    event.preventDefault();
    if (key === "End") {
      this.jumpToLatest();
    } else if (this.canScrollUp()) {
      this.glideTo(0);
      this.leave();
    }
  };

  private wheelScrollsUp(event: WheelEvent): boolean {
    if (!(event.deltaY < 0) || event.ctrlKey || event.shiftKey) return false;
    if (Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return false;
    return this.canScrollUp();
  }

  private keyScrollsUp(event: KeyboardEvent): boolean {
    if (event.defaultPrevented || event.altKey) return false;
    const up =
      UP_KEYS.has(event.key) || (event.key === " " && event.shiftKey && event.target === this.el);
    return up && this.el !== null && keyReachesFeed(event.target, this.el) && this.canScrollUp();
  }

  private canScrollUp(): boolean {
    return this.el !== null && this.el.scrollTop > 0;
  }

  private leave(): void {
    this.following = false;
    this.refreshPill();
  }

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
      if (reshaped || roomCaughtUp) this.pinToBottom();
      else this.keepLastLineClear();
    } else if (!this.held) {
      this.refreshPill();
    }
  };

  attach(el: HTMLElement, content: HTMLElement): () => void {
    this.el = el;
    this.content = content;
    const resizes = new ResizeObserver(this.onResize);
    resizes.observe(content, { box: "border-box" });
    resizes.observe(el);
    el.addEventListener("scroll", this.onScroll, { passive: true });
    for (const type of READER_INPUT_EVENTS) {
      el.addEventListener(type, this.onReaderInput, { passive: true });
    }
    // Not passive: it takes Home and End over from the browser.
    el.addEventListener("keydown", this.onKeyDown);
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
      for (const type of READER_INPUT_EVENTS) el.removeEventListener(type, this.onReaderInput);
      el.removeEventListener("keydown", this.onKeyDown);
      el.removeEventListener("click", this.onPress, true);
      this.letGoOfPlace();
      this.stopGlide();
      if (this.el === el) {
        this.el = null;
        this.content = null;
      }
    };
  }

  get isFollowing(): boolean {
    return this.following;
  }

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

  hold(): void {
    this.held = true;
    this.following = false;
    this.scrolledUp = true;
  }

  release(): void {
    if (!this.held) return;
    this.held = false;
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.know(el.scrollTop);
    this.following = this.nearEnd();
    this.refreshPill();
  }

  get isHeld(): boolean {
    return this.held;
  }

  jumpToLatest(): void {
    const el = this.el;
    if (!el) return;
    this.letGoOfPlace();
    this.following = true;
    this.held = false;
    this.scrolledUp = false;
    this.unseen = false;
    this.glideTo("end");
  }

  private glideTo(target: 0 | "end"): void {
    const el = this.el;
    if (!el) return;
    const destination = (): number =>
      target === "end" ? Math.max(0, el.scrollHeight - el.clientHeight) : target;
    if (prefersReducedMotion()) {
      this.write(target === "end" ? el.scrollHeight : target);
      return;
    }
    this.stopGlide();
    const from = el.scrollTop;
    this.know(from);
    const duration = glideDuration(Math.abs(destination() - from));
    let start: number | null = null;
    const step = (now: number): void => {
      if (this.el !== el || isHidden(el)) {
        this.glideFrame = 0;
        return;
      }
      if (this.knownTop !== null && Math.abs(el.scrollTop - this.knownTop) >= 1) {
        this.takeInScroll();
        this.glideFrame = 0;
        return;
      }
      this.glideFrame = 0;
      start ??= now - FRAME_MS;
      const progress = Math.min(1, (now - start) / duration);
      const to = destination();
      this.place(progress < 1 ? from + (to - from) * easeOut(progress) : to);
      if (!this.following) this.refreshPill();
      if (progress < 1) this.glideFrame = requestAnimationFrame(step);
    };
    this.glideFrame = requestAnimationFrame(step);
  }

  private stopGlide(): void {
    if (this.glideFrame === 0) return;
    cancelAnimationFrame(this.glideFrame);
    this.glideFrame = 0;
  }

  private interruptGlide(): void {
    if (this.glideFrame === 0) return;
    this.stopGlide();
    if (this.held) return;
    if (this.following && !this.nearEnd()) this.following = false;
    this.refreshPill();
  }

  private pinToBottom(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.write(el.scrollHeight);
    this.pinnedCovered = this.covered();
    this.scrolledUp = false;
    this.unseen = false;
  }

  private bottomRoom(): number {
    const content = this.content;
    if (!content) return 0;
    return Number.parseFloat(getComputedStyle(content).paddingBottom) || 0;
  }

  private contentEnd(): number {
    const el = this.el;
    return el === null ? 0 : el.scrollHeight - this.bottomRoom();
  }

  private reach(): number {
    const el = this.el;
    if (!el) return 0;
    return this.contentEnd() - (el.scrollTop + el.clientHeight - this.covered());
  }

  private atBottom(): boolean {
    const el = this.el;
    if (!el) return true;
    return el.scrollHeight - el.scrollTop - el.clientHeight <= SCROLL_END_SLACK_PX;
  }

  private nearEnd(): boolean {
    return this.atBottom() || this.reach() <= this.followWithinPx;
  }

  private keepLastLineClear(): void {
    const el = this.el;
    if (!el) return;
    const reach = this.reach();
    if (reach > 0) this.write(el.scrollTop + reach);
  }

  private refreshPill(): void {
    const el = this.el;
    if (!el || isHidden(el)) return;
    this.scrolledUp = !this.following && !this.nearEnd();
    if (!this.scrolledUp) this.unseen = false;
  }

  private keepPlaceOf(control: HTMLElement): void {
    const el = this.el;
    const content = this.content;
    if (!el || !content) return;
    this.following = false;
    const item = control.closest<HTMLElement>("[data-feed-item]");
    this.keeping = {
      control,
      controls: control.getAttribute("aria-controls"),
      position: [...(item ?? el).querySelectorAll(EXPANDERS)].indexOf(control),
      item,
      controlTop: placeIn(el, control),
      itemTop: item === null ? 0 : offsetIn(el, item),
    };
    content.style.minHeight = `${String(content.offsetHeight)}px`;
    this.holdingHeight = true;
    clearTimeout(this.keepingTimer);
    this.keepingTimer = setTimeout(() => {
      this.keeping = null;
    }, KEEP_PLACE_MS);
    cancelAnimationFrame(this.keepingFrame);
    this.keepingFrame = requestAnimationFrame(this.keepPlaceEachFrame);
  }

  private readonly keepPlaceEachFrame = (): void => {
    if (this.keeping === null) return;
    this.restorePlace();
    this.keepingFrame = requestAnimationFrame(this.keepPlaceEachFrame);
  };

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
