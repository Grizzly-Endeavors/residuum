import { afterEach, describe, expect, it, vi } from "vitest";
import { FeedScroller, type FeedScrollerOptions } from "./feed-scroll.svelte";

/** A ResizeObserver whose notifications the test sends. */
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
  /** The content's bottom padding: room reserved under its last line. */
  padding: number;
}

interface FakeScroller {
  el: HTMLElement;
  content: HTMLElement;
  scrollTo: ReturnType<typeof vi.fn>;
  /** Move the scroll position the way the reader does, and tell the scroller. */
  scrollBy: (top: number) => void;
  /** Press a key in the feed. */
  press: (init: KeyboardEventInit) => void;
  geometry: Geometry;
}

function fakeScroller(): FakeScroller {
  const geometry: Geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1500, padding: 0 };
  const listeners = new Map<string, (event?: Event) => void>();
  const scrollTo = vi.fn((options: { top: number }) => {
    geometry.scrollTop = options.top;
  });
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
    scrollBy: (top) => {
      geometry.scrollTop = top;
      listeners.get("scroll")?.();
    },
    press: (init) => {
      listeners.get("keydown")?.({ type: "keydown", ...init } as unknown as Event);
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

  it("follows the newest content from End, though content lands at the top on the way", () => {
    const { feed, observer } = attached();
    // The reader went to the top, so the feed no longer follows.
    feed.scrollBy(0);

    feed.press({ key: "End" });
    // The browser's glide is partway down when an older episode is prepended.
    feed.scrollBy(600);
    feed.geometry.scrollHeight = 3000;
    observer.notify();

    expect(feed.geometry.scrollTop).toBe(3000);
  });

  it("leaves End with a modifier, and other keys, to the browser alone", () => {
    const { feed, observer } = attached();
    feed.scrollBy(0);

    feed.press({ key: "End", shiftKey: true });
    feed.press({ key: "PageDown" });
    feed.geometry.scrollHeight = 3000;
    observer.notify();

    expect(feed.geometry.scrollTop).toBe(0);
  });

  it("watches the feed's content and its viewport", () => {
    const { feed, observer } = attached();
    expect(observer.observed).toEqual([feed.content, feed.el]);
  });

  it("keeps a following reader at the bottom when the viewport shrinks under them", () => {
    const { feed, observer } = attached();
    feed.scrollTo.mockClear();
    // The window narrowed, so the same content has less room.
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

  describe("how near the end counts as following", () => {
    it("measures from the end of the content, not from the end of the room reserved below it", () => {
      const { feed, scroller } = attached();
      // 300px of room under the last line: the content ends at 1700.
      feed.geometry.padding = 300;
      feed.scrollBy(1500);
      expect(scroller.scrolledUp).toBe(false);

      // 400px from the true bottom, but the last line is 100px under the view's foot.
      feed.scrollBy(1100);
      expect(scroller.scrolledUp).toBe(false);
      expect(scroller.isFollowing).toBe(true);

      // 121px under it: far enough to stop following.
      feed.scrollBy(1079);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.isFollowing).toBe(false);
    });

    it("sees the last line only above what floats over the foot of the area", () => {
      let covered = 150;
      const { feed, scroller } = attached({ covered: () => covered });
      feed.geometry.padding = 150;
      feed.scrollBy(1500);
      expect(scroller.scrolledUp).toBe(false);

      // The same position is worse with a taller composer: its 400px hide the last 250px of text.
      covered = 400;
      feed.scrollBy(1500);
      expect(scroller.scrolledUp).toBe(true);
    });
  });

  describe("when the composer over the foot changes size", () => {
    function composerFeed(): ReturnType<typeof attached> & { grow: (by: number) => void } {
      let covered = 100;
      const result = attached({ covered: () => covered });
      // The reserved room is the composer plus a 200px buffer, and the reader sits at the true bottom.
      result.feed.geometry.padding = 300;
      result.observer.notify();
      result.feed.geometry.scrollTop = 1500;
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
      // Nor does it flip the feed out of following, whatever the composer grew by.
      expect(scroller.scrolledUp).toBe(false);
      expect(scroller.isFollowing).toBe(true);
    });

    it("moves the thread only as far as keeps the last line out from under it", () => {
      const { feed, observer, grow } = composerFeed();
      // 200px of buffer is gone after 200px of growth, which leaves 200px of text covered.
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
      expect(feed.scrollTo).toHaveBeenCalledWith({ top: 2000, behavior: "smooth" });
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
});
