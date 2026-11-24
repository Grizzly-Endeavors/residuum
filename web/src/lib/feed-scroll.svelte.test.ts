import { afterEach, describe, expect, it, vi } from "vitest";
import { FeedScroller, type FeedScrollerOptions } from "./feed-scroll.svelte";

class ManualObserver {
  static instances: ManualObserver[] = [];
  readonly observed: unknown[] = [];

  constructor(private readonly callback: () => void) {
    ManualObserver.instances.push(this);
  }

  observe(target: unknown): void {
    this.observed.push(target);
  }

  disconnect(): void {}

  notify(): void {
    this.callback();
  }
}

interface Geometry {
  scrollHeight: number;
  clientHeight: number;
  scrollTop: number;
  padding: number;
}

interface FakeScroller {
  el: HTMLElement;
  content: HTMLElement;
  scrollTo: ReturnType<typeof vi.fn>;
  scrollBy: (top: number) => void;
  press: (init: KeyboardEventInit & { defaultPrevented?: boolean }) => ReturnType<typeof vi.fn>;
  fire: (type: string, init?: Record<string, unknown>) => void;
  frames: (count: number) => void;
  finishGlide: () => void;
  seen: number[];
  geometry: Geometry;
}

function fakeScroller(): FakeScroller {
  const geometry: Geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1500, padding: 0 };
  const listeners = new Map<string, (event?: Event) => void>();
  const seen: number[] = [];
  const scrollTo = vi.fn((options: { top: number; behavior?: string }) => {
    geometry.scrollTop = options.top;
  });
  const waiting = new Map<number, FrameRequestCallback>();
  let nextFrame = 1;
  let frameTime = 1000;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback): number => {
    waiting.set(nextFrame, callback);
    return nextFrame++;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => waiting.delete(id));
  const scrolled = (): void => {
    seen.push(geometry.scrollTop);
    listeners.get("scroll")?.();
  };
  const frames = (count: number): void => {
    for (let n = 0; n < count; n++) {
      const due = [...waiting.values()];
      waiting.clear();
      frameTime += 16;
      const before = geometry.scrollTop;
      for (const callback of due) callback(frameTime);
      if (geometry.scrollTop !== before) scrolled();
    }
  };
  const el = {
    get scrollHeight() {
      return geometry.scrollHeight;
    },
    get clientHeight() {
      return geometry.clientHeight;
    },
    get scrollTop() {
      return geometry.scrollTop;
    },
    addEventListener: (type: string, listener: (event?: Event) => void) =>
      listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
    scrollTo,
  } as unknown as HTMLElement;
  vi.stubGlobal("getComputedStyle", () => ({ paddingBottom: `${String(geometry.padding)}px` }));
  return {
    el,
    content: {} as HTMLElement,
    scrollTo,
    geometry,
    seen,
    scrollBy: (top) => {
      geometry.scrollTop = top;
      scrolled();
    },
    press: (init) => {
      const preventDefault = vi.fn();
      listeners.get("keydown")?.({
        type: "keydown",
        target: el,
        defaultPrevented: false,
        preventDefault,
        ...init,
      } as unknown as Event);
      return preventDefault;
    },
    fire: (type, init = {}) => {
      listeners.get(type)?.({ type, ...init } as unknown as Event);
    },
    frames,
    finishGlide: () => {
      for (let n = 0; waiting.size > 0; n++) {
        if (n > 100) throw new Error("the glide is still going after 100 frames");
        frames(1);
      }
    },
  };
}

afterEach(() => {
  ManualObserver.instances = [];
  vi.unstubAllGlobals();
});

describe("FeedScroller", () => {
  function attached(options?: FeedScrollerOptions): {
    feed: FakeScroller;
    observer: ManualObserver;
    scroller: FeedScroller;
  } {
    vi.stubGlobal("ResizeObserver", ManualObserver);
    const feed = fakeScroller();
    const scroller = new FeedScroller(options);
    scroller.attach(feed.el, feed.content);
    const observer = ManualObserver.instances[0];
    if (observer === undefined) throw new Error("the scroller made no ResizeObserver");
    return { feed, observer, scroller };
  }

  it("takes End over and glides to the newest content, as Jump to latest does", () => {
    const { feed, scroller } = attached();
    feed.scrollBy(100);
    expect(scroller.scrolledUp).toBe(true);

    const preventDefault = feed.press({ key: "End" });

    expect(preventDefault).toHaveBeenCalled();
    expect(scroller.isFollowing).toBe(true);
    expect(scroller.scrolledUp).toBe(false);
    feed.frames(4);
    expect(feed.geometry.scrollTop).toBeGreaterThan(100);
    expect(feed.geometry.scrollTop).toBeLessThan(1500);
    feed.finishGlide();
    expect(feed.geometry.scrollTop).toBe(1500);
    expect(feed.scrollTo).not.toHaveBeenCalledWith(expect.objectContaining({ behavior: "smooth" }));
    expect(scroller.isFollowing).toBe(true);
  });

  it("follows the newest content from End, though content lands at the top on the way", () => {
    const { feed, observer } = attached();
    feed.scrollBy(0);

    feed.press({ key: "End" });
    feed.frames(4);
    feed.geometry.scrollHeight = 3000;
    observer.notify();
    expect(feed.geometry.scrollTop).toBe(3000);

    feed.finishGlide();
    expect(feed.geometry.scrollTop).toBe(3000);
  });

  it("ends End's glide at the end as it is when the glide gets there", () => {
    const { feed } = attached();
    feed.scrollBy(0);
    feed.press({ key: "End" });
    feed.frames(4);
    feed.geometry.scrollHeight = 3000;
    feed.finishGlide();
    expect(feed.geometry.scrollTop).toBe(2500);
  });

  it("follows from End pressed during Home's glide, while an older episode loads", () => {
    // Seen in Chromium (#756): the browser's glide for Home went on after End,
    // so End's started up and read as the reader leaving, and the older episode
    // landing then kept the reader's place, which ended End's glide near the top.
    const { feed, scroller } = attached();
    feed.press({ key: "Home" });
    feed.frames(8);
    const atEnd = feed.geometry.scrollTop;
    expect(atEnd).toBeLessThan(1500);
    expect(atEnd).toBeGreaterThan(0);
    const seenBefore = feed.seen.length;

    feed.press({ key: "End" });
    feed.frames(3);
    scroller.takeInPendingScroll();
    feed.geometry.scrollHeight = 2322;
    expect(scroller.isFollowing).toBe(true);
    scroller.contentChanged();
    feed.finishGlide();

    const afterEnd = feed.seen.slice(seenBefore);
    for (const [n, top] of afterEnd.entries()) {
      expect(top).toBeGreaterThanOrEqual(n === 0 ? atEnd : (afterEnd[n - 1] ?? 0));
    }
    expect(feed.geometry.scrollTop).toBe(2322);
    expect(scroller.isFollowing).toBe(true);
    expect(scroller.scrolledUp).toBe(false);
  });

  it("takes Home over: it glides to the top, and the feed stops following", () => {
    const { feed, observer, scroller } = attached();

    const preventDefault = feed.press({ key: "Home" });

    expect(preventDefault).toHaveBeenCalled();
    expect(scroller.isFollowing).toBe(false);
    feed.finishGlide();
    expect(feed.geometry.scrollTop).toBe(0);
    expect(scroller.scrolledUp).toBe(true);
    feed.geometry.scrollHeight = 2100;
    observer.notify();
    expect(feed.geometry.scrollTop).toBe(0);
  });

  it("goes to the top and the bottom at once from Home and End under reduced motion", () => {
    vi.stubGlobal("window", {
      matchMedia: (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)" }),
    });
    const { feed } = attached();
    feed.press({ key: "Home" });
    expect(feed.scrollTo).toHaveBeenLastCalledWith({ top: 0, behavior: "instant" });
    feed.press({ key: "End" });
    expect(feed.scrollTo).toHaveBeenLastCalledWith({ top: 2000, behavior: "instant" });
  });

  it("shows the pill after Home from the bottom under reduced motion, where the jump is at once", () => {
    vi.stubGlobal("window", {
      matchMedia: (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)" }),
    });
    const { feed, scroller } = attached();
    expect(scroller.scrolledUp).toBe(false);

    feed.press({ key: "Home" });
    // The browser's scroll event for the jump arrives after the position is already at the top.
    feed.scrollBy(0);

    expect(scroller.isFollowing).toBe(false);
    expect(scroller.scrolledUp).toBe(true);
  });

  it("keeps following at Home with nowhere above to go", () => {
    const { feed, scroller } = attached();
    feed.geometry.scrollHeight = 500;
    feed.geometry.scrollTop = 0;
    feed.press({ key: "Home" });
    expect(feed.scrollTo).not.toHaveBeenCalled();
    expect(scroller.isFollowing).toBe(true);
  });

  it("counts a move up after End at the bottom as the reader's", () => {
    const { feed, scroller } = attached();
    feed.press({ key: "End" });
    feed.scrollBy(1300);
    expect(scroller.isFollowing).toBe(false);
    feed.finishGlide();
    expect(feed.geometry.scrollTop).toBe(1300);
  });

  it("leaves a reader who stops End's glide short with a wheel, a touch or a press where it stopped, not following", () => {
    for (const type of ["wheel", "touchstart", "pointerdown"]) {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(0);
      feed.press({ key: "End" });
      feed.frames(3);
      const reached = feed.geometry.scrollTop;
      expect(reached).toBeLessThan(1500 - 120);
      feed.fire(type, { deltaY: 10, deltaX: 0 });
      feed.finishGlide();
      expect(feed.geometry.scrollTop).toBe(reached);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(true);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(reached);
    }
  });

  it("leaves a reader moved down short of the end during End's glide there, not following", () => {
    const { feed, observer, scroller } = attached();
    feed.scrollBy(0);
    feed.press({ key: "End" });
    feed.frames(3);
    const moved = feed.geometry.scrollTop + 100;
    expect(moved).toBeLessThan(1500 - 120);
    feed.scrollBy(moved);
    feed.finishGlide();
    expect(feed.geometry.scrollTop).toBe(moved);
    expect(scroller.isFollowing).toBe(false);
    expect(scroller.scrolledUp).toBe(true);
    feed.geometry.scrollHeight = 2100;
    observer.notify();
    expect(feed.geometry.scrollTop).toBe(moved);
  });

  it("keeps following a reader moved to near the end during End's glide", () => {
    const { feed, observer, scroller } = attached();
    feed.scrollBy(0);
    feed.press({ key: "End" });
    feed.frames(3);
    feed.scrollBy(1500 - 60);
    feed.finishGlide();
    expect(scroller.isFollowing).toBe(true);
    expect(scroller.scrolledUp).toBe(false);
    feed.geometry.scrollHeight = 2100;
    observer.notify();
    expect(feed.geometry.scrollTop).toBe(2100);
  });

  it("moves on the glide's first frame", () => {
    const { feed } = attached();
    feed.scrollBy(0);
    feed.press({ key: "End" });
    feed.frames(1);
    expect(feed.geometry.scrollTop).toBeGreaterThan(0);
  });

  it("leaves Home and End with a modifier, and other keys, to the browser", () => {
    const { feed, observer, scroller } = attached();
    feed.scrollBy(0);

    const pressed = [
      feed.press({ key: "End", shiftKey: true }),
      feed.press({ key: "End", ctrlKey: true }),
      feed.press({ key: "End", metaKey: true }),
      feed.press({ key: "End", altKey: true }),
      feed.press({ key: "PageDown" }),
    ];
    feed.geometry.scrollHeight = 3000;
    observer.notify();

    for (const preventDefault of pressed) expect(preventDefault).not.toHaveBeenCalled();
    expect(feed.scrollTo).not.toHaveBeenCalled();
    expect(feed.geometry.scrollTop).toBe(0);
    expect(scroller.isFollowing).toBe(false);
  });

  it("still stops following at Home with a modifier, which the browser scrolls up", () => {
    const { feed, scroller } = attached();
    const preventDefault = feed.press({ key: "Home", ctrlKey: true });
    expect(preventDefault).not.toHaveBeenCalled();
    expect(feed.scrollTo).not.toHaveBeenCalled();
    expect(scroller.isFollowing).toBe(false);
  });

  it("leaves Home and End that something in the feed handled to it", () => {
    const { feed, scroller } = attached();
    feed.scrollBy(100);
    feed.press({ key: "End", defaultPrevented: true });
    feed.press({ key: "Home", defaultPrevented: true });
    expect(feed.scrollTo).not.toHaveBeenCalled();
    expect(scroller.isFollowing).toBe(false);
  });

  it("watches the feed's content and its viewport", () => {
    const { feed, observer } = attached();
    expect(observer.observed).toEqual([feed.content, feed.el]);
  });

  it("keeps a following reader at the bottom when the viewport shrinks under them", () => {
    const { feed, observer } = attached();
    feed.scrollTo.mockClear();
    feed.geometry.clientHeight = 470;
    observer.notify();
    expect(feed.scrollTo).toHaveBeenCalledWith({ top: 2000, behavior: "instant" });
  });

  it("leaves a reader who scrolled up where they are when the viewport changes", () => {
    const { feed, observer } = attached();
    feed.scrollBy(100);
    feed.scrollTo.mockClear();
    feed.geometry.clientHeight = 470;
    observer.notify();
    expect(feed.scrollTo).not.toHaveBeenCalled();
  });

  it("pins a following reader to the true bottom, the reserved room included", () => {
    const { feed, observer, scroller } = attached();
    feed.geometry.padding = 300;
    feed.scrollTo.mockClear();
    feed.geometry.scrollHeight = 2400;
    observer.notify();
    expect(feed.scrollTo).toHaveBeenCalledWith({ top: 2400, behavior: "instant" });
    scroller.contentChanged();
    expect(feed.geometry.scrollTop).toBe(2400);
  });

  describe("how near the end counts as back at the end", () => {
    it("measures from the end of the content, not from the end of the room reserved below it", () => {
      const { feed, scroller } = attached();
      feed.geometry.padding = 300;
      feed.scrollBy(0);
      expect(scroller.isFollowing).toBe(false);

      feed.scrollBy(1079);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.isFollowing).toBe(false);

      feed.scrollBy(1100);
      expect(scroller.scrolledUp).toBe(false);
      expect(scroller.isFollowing).toBe(true);
    });

    it("sees the last line only above what floats over the foot of the area", () => {
      let covered = 150;
      const { feed, scroller } = attached({ covered: () => covered });
      feed.geometry.padding = 150;
      feed.scrollBy(1450);
      expect(scroller.scrolledUp).toBe(false);

      covered = 400;
      feed.scrollBy(1451);
      expect(scroller.scrolledUp).toBe(true);
    });

    it("keeps a reader at the very bottom following while the room for the composer catches up", () => {
      const { feed, scroller } = attached({ covered: () => 137 });
      feed.geometry.padding = 48;
      feed.scrollBy(1400);
      feed.scrollBy(1500);
      expect(scroller.isFollowing).toBe(true);
      expect(scroller.scrolledUp).toBe(false);
    });

    it("doesn't pull back a reader whose scroll lands before its event does", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(1500);
      expect(scroller.isFollowing).toBe(true);

      feed.scrollTo.mockClear();
      feed.geometry.scrollTop = 600;
      feed.geometry.scrollHeight = 2100;
      scroller.contentChanged();
      observer.notify();

      expect(feed.scrollTo).not.toHaveBeenCalled();
      expect(feed.geometry.scrollTop).toBe(600);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(true);
    });

    it("keeps a pinned reader at the bottom when the room for an unchanged composer lands late", () => {
      const { feed, observer, scroller } = attached({ covered: () => 109 });
      feed.geometry.padding = 212;
      observer.notify();
      scroller.contentChanged(true);
      expect(feed.geometry.scrollTop).toBe(2000);

      feed.scrollTo.mockClear();
      feed.geometry.padding = 321;
      feed.geometry.scrollHeight = 2109;
      observer.notify();
      expect(feed.scrollTo).toHaveBeenCalledWith({ top: 2109, behavior: "instant" });
    });

    it("doesn't take the late event of its own scroll for the reader leaving the end", () => {
      const { feed, observer, scroller } = attached({ covered: () => 137 });
      feed.geometry.padding = 48;
      feed.scrollTo.mockClear();
      scroller.contentChanged(true);
      expect(feed.geometry.scrollTop).toBe(2000);
      feed.geometry.scrollHeight = 2546;
      feed.scrollBy(feed.geometry.scrollTop);
      expect(scroller.isFollowing).toBe(true);

      feed.geometry.scrollHeight = 3500;
      observer.notify();
      expect(feed.scrollTo).toHaveBeenLastCalledWith({ top: 3500, behavior: "instant" });
      expect(scroller.scrolledUp).toBe(false);
    });
  });

  describe("when the composer over the foot changes size", () => {
    function composerFeed(): ReturnType<typeof attached> & { grow: (by: number) => void } {
      let covered = 100;
      const result = attached({ covered: () => covered });
      result.feed.geometry.padding = 300;
      result.observer.notify();
      result.feed.scrollBy(1500);
      result.feed.scrollTo.mockClear();
      return {
        ...result,
        grow: (by) => {
          covered += by;
          result.feed.geometry.padding += by;
          result.feed.geometry.scrollHeight += by;
        },
      };
    }

    it("doesn't move a following reader's thread while the buffer absorbs it", () => {
      const { feed, observer, scroller, grow } = composerFeed();
      grow(150);
      observer.notify();
      feed.scrollBy(feed.geometry.scrollTop);

      expect(feed.scrollTo).not.toHaveBeenCalled();
      expect(feed.geometry.scrollTop).toBe(1500);
      expect(scroller.scrolledUp).toBe(false);
      expect(scroller.isFollowing).toBe(true);
    });

    it("moves the thread only as far as keeps the last line out from under it", () => {
      const { feed, observer, grow } = composerFeed();
      grow(400);
      observer.notify();

      expect(feed.scrollTo).toHaveBeenCalledOnce();
      expect(feed.scrollTo).toHaveBeenCalledWith({ top: 1700, behavior: "instant" });
    });

    it("doesn't move a following reader's thread when it shrinks", () => {
      const { feed, observer, grow } = composerFeed();
      grow(-50);
      observer.notify();
      expect(feed.scrollTo).not.toHaveBeenCalled();
    });

    it("leaves a reader who scrolled up where they are, grown or shrunk", () => {
      const { feed, observer, scroller, grow } = composerFeed();
      feed.scrollBy(600);
      grow(250);
      observer.notify();
      grow(-250);
      observer.notify();
      expect(feed.scrollTo).not.toHaveBeenCalled();
      expect(feed.geometry.scrollTop).toBe(600);
      expect(scroller.scrolledUp).toBe(true);
    });
  });

  describe("new content below a reader who scrolled up", () => {
    it("is flagged as unseen when an item arrives, until they are back at the end", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.unseen).toBe(false);

      feed.geometry.scrollHeight = 2400;
      scroller.contentChanged(false, true);
      expect(scroller.unseen).toBe(true);
      expect(feed.scrollTo).not.toHaveBeenCalled();

      feed.scrollBy(1900);
      expect(scroller.scrolledUp).toBe(false);
      expect(scroller.unseen).toBe(false);
    });

    it("isn't flagged by content that only grew, such as a reply filling in", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      scroller.contentChanged(false, false);
      expect(scroller.unseen).toBe(false);
    });

    it("isn't flagged for a following reader, who is taken down to it", () => {
      const { feed, scroller } = attached();
      feed.geometry.scrollHeight = 2400;
      scroller.contentChanged(false, true);
      expect(scroller.unseen).toBe(false);
      expect(feed.geometry.scrollTop).toBe(2400);
    });

    it("is cleared by Jump to latest", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      scroller.contentChanged(false, true);
      expect(scroller.unseen).toBe(true);
      scroller.jumpToLatest();
      expect(scroller.unseen).toBe(false);
      expect(scroller.scrolledUp).toBe(false);
    });
  });

  describe("Jump to latest", () => {
    it("glides to the newest content", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      feed.frames(4);
      expect(feed.geometry.scrollTop).toBeLessThan(1500);
      feed.finishGlide();
      expect(feed.geometry.scrollTop).toBe(1500);
      expect(scroller.isFollowing).toBe(true);
    });

    it("jumps at once under reduced motion", () => {
      vi.stubGlobal("window", {
        matchMedia: (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)" }),
      });
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      expect(feed.scrollTo).toHaveBeenCalledWith({ top: 2000, behavior: "instant" });
    });
  });

  describe("leaving the end", () => {
    it("leaves a reader who has started scrolling up where they are as a streamed reply lands", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(1460);
      feed.geometry.scrollHeight = 2025;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(1460);
      expect(scroller.isFollowing).toBe(false);
    });

    it("stops following at a wheel turned up, before the position has moved", () => {
      const { feed, observer, scroller } = attached();
      feed.fire("wheel", { deltaY: -100 });
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(false);

      feed.geometry.scrollHeight = 2025;
      observer.notify();
      expect(feed.scrollTo).not.toHaveBeenCalled();
    });

    it("a slow drag up of 1px a frame during a stream is the reader leaving", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(1499);
      feed.geometry.scrollHeight = 2025;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(1499);
      expect(scroller.isFollowing).toBe(false);
    });

    it("a wheel turned up stays left after its first 1px of scroll", () => {
      const { feed, observer, scroller } = attached();
      feed.fire("wheel", { deltaY: -1 });
      feed.scrollBy(1499);
      expect(scroller.isFollowing).toBe(false);
      feed.geometry.scrollHeight = 2025;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(1499);
    });

    it("keeps a reader at the bottom following when the content shrinks 1px under them", () => {
      const { feed, observer, scroller } = attached();
      feed.geometry.scrollHeight = 1999;
      feed.scrollBy(1499);
      expect(scroller.isFollowing).toBe(true);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
    });

    it("keeps following through a wheel turned down, sideways or to zoom", () => {
      const { feed, scroller } = attached();
      feed.fire("wheel", { deltaY: 100 });
      feed.fire("wheel", { deltaY: -2, deltaX: 40 });
      feed.fire("wheel", { deltaY: -100, shiftKey: true });
      feed.fire("wheel", { deltaY: -100, ctrlKey: true });
      expect(scroller.isFollowing).toBe(true);
    });

    it("stops following at a key that scrolls up, and not at one something in the feed handled", () => {
      const { feed, scroller } = attached();
      feed.press({ key: "PageDown" });
      expect(scroller.isFollowing).toBe(true);
      feed.press({ key: "ArrowUp", defaultPrevented: true });
      expect(scroller.isFollowing).toBe(true);
      feed.press({ key: "PageUp" });
      expect(scroller.isFollowing).toBe(false);
    });

    it("respects a move up with no input in the feed, as find in page or a screen reader makes", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(1440);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(false);

      feed.geometry.scrollHeight = 2100;
      observer.notify();
      scroller.contentChanged(false, true);
      expect(feed.scrollTo).not.toHaveBeenCalled();
      expect(feed.geometry.scrollTop).toBe(1440);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.unseen).toBe(true);
    });

    it("doesn't take the browser's move while the page changes the items for the reader's", () => {
      const { feed, scroller } = attached();
      scroller.takeInPendingScroll();
      // As the items change, the browser moves the position up 36px at a layout
      // in between, and then an older episode is in.
      feed.geometry.scrollTop = 1464;
      feed.geometry.scrollHeight = 2800;
      scroller.contentChanged();
      expect(scroller.isFollowing).toBe(true);
      expect(feed.geometry.scrollTop).toBe(2800);
      expect(scroller.scrolledUp).toBe(false);
    });

    it("takes a move found after a frame has passed for the reader's again", () => {
      const { feed, observer, scroller } = attached();
      scroller.takeInPendingScroll();
      observer.notify();
      feed.geometry.scrollTop = 1300;
      scroller.contentChanged();
      expect(scroller.isFollowing).toBe(false);
      expect(feed.geometry.scrollTop).toBe(1300);
    });

    it("doesn't take the browser keeping a reader at the bottom in range for them leaving", () => {
      const { feed, observer, scroller } = attached();
      feed.geometry.clientHeight = 530;
      feed.scrollBy(1470);
      expect(scroller.isFollowing).toBe(true);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
    });

    it("doesn't take it for them leaving when content grows in place before the move's event", () => {
      const { feed, observer, scroller } = attached();
      feed.geometry.clientHeight = 530;
      feed.geometry.scrollHeight = 2240;
      feed.scrollBy(1470);
      expect(scroller.isFollowing).toBe(true);
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2240);
      expect(scroller.scrolledUp).toBe(false);
    });

    it("still takes a move up for the reader's when content grew in place before its event", () => {
      const { feed, scroller } = attached();
      feed.geometry.scrollHeight = 2240;
      feed.scrollBy(1490);
      expect(scroller.isFollowing).toBe(false);
    });
  });

  describe("a flick", () => {
    it("counts the momentum after a flick as the reader's", () => {
      const { feed, observer, scroller } = attached();
      feed.fire("touchstart");
      feed.scrollBy(1450);
      feed.fire("touchend", { touches: [] });
      expect(scroller.isFollowing).toBe(false);

      feed.scrollBy(1380);
      feed.geometry.scrollHeight = 2025;
      observer.notify();
      feed.scrollBy(1320);
      feed.geometry.scrollHeight = 2050;
      observer.notify();

      expect(feed.scrollTo).not.toHaveBeenCalled();
      expect(feed.geometry.scrollTop).toBe(1320);
      expect(scroller.isFollowing).toBe(false);
    });

    it("follows again when momentum carries the reader back to the end", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(400);
      feed.fire("touchstart");
      feed.scrollBy(900);
      feed.fire("touchend", { touches: [] });
      feed.scrollBy(1500);
      expect(scroller.isFollowing).toBe(true);
      feed.geometry.scrollHeight = 2025;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2025);
    });
  });

  describe("the glide to the latest", () => {
    it("doesn't count its own scroll events as the reader's", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      feed.frames(6);
      expect(feed.seen.length).toBeGreaterThan(2);
      expect(scroller.isFollowing).toBe(true);
      expect(scroller.scrolledUp).toBe(false);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
    });

    it("counts a reader who scrolls away after Jump to latest was cut short by new content", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(100);
      expect(scroller.scrolledUp).toBe(true);

      scroller.jumpToLatest();
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
      feed.scrollBy(feed.geometry.scrollTop);

      feed.scrollBy(0);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.isFollowing).toBe(false);

      feed.geometry.scrollHeight = 2200;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(0);
      feed.finishGlide();
      expect(feed.geometry.scrollTop).toBe(0);
    });

    it("counts a move up while it runs as the reader's, and leaves them there", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      feed.frames(4);
      feed.scrollBy(0);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(true);
      feed.finishGlide();
      expect(feed.geometry.scrollTop).toBe(0);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(0);
    });

    it("leaves a reader moved between two of its frames where they went, before that move's event", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      feed.frames(4);
      feed.geometry.scrollTop = 0;
      feed.frames(1);
      expect(feed.geometry.scrollTop).toBe(0);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(true);
      feed.finishGlide();
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(0);
    });

    it("ends when the feed is hidden and shown again between two of its frames", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(0);
      scroller.jumpToLatest();
      feed.frames(3);
      feed.geometry.clientHeight = 0;
      observer.notify();
      feed.geometry.clientHeight = 500;
      feed.geometry.scrollTop = 700;
      feed.frames(1);
      expect(feed.geometry.scrollTop).toBe(700);
      feed.fire("touchstart");
      expect(scroller.isFollowing).toBe(true);
    });

    it("leaves the pill to a rebuild that holds the reader's state when a move ends it", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      feed.frames(3);
      scroller.hold();
      expect(scroller.scrolledUp).toBe(true);
      feed.geometry.scrollTop = 1450;
      feed.frames(1);
      expect(feed.geometry.scrollTop).toBe(1450);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.isHeld).toBe(true);
    });

    it("stops when the feed is hidden, and doesn't move it while it is", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      scroller.jumpToLatest();
      feed.frames(3);
      const reached = feed.geometry.scrollTop;
      feed.geometry.clientHeight = 0;
      feed.finishGlide();
      expect(feed.geometry.scrollTop).toBe(reached);
    });
  });
});
