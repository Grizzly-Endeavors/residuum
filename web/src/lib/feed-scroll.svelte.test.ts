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
  /** Send the feed an event, as the browser does for the reader's input. */
  fire: (type: string, init?: Record<string, unknown>) => void;
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
    fire: (type, init = {}) => {
      listeners.get(type)?.({ type, ...init } as unknown as Event);
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

  it("follows from End though the browser's glide first carries on up from Home", () => {
    const { feed, observer, scroller } = attached();
    feed.press({ key: "Home" });
    feed.scrollBy(900);
    // End, while the scroll for Home is still under way: it carries on up a little.
    feed.press({ key: "End" });
    feed.scrollBy(860);
    expect(scroller.isFollowing).toBe(true);
    expect(scroller.scrolledUp).toBe(false);
    feed.scrollBy(1200);
    feed.geometry.scrollHeight = 2800;
    observer.notify();
    expect(feed.geometry.scrollTop).toBe(2800);
  });

  it("starts no glide from End at the bottom, so a later move up is the reader's", () => {
    const { feed, scroller } = attached();
    feed.press({ key: "End" });
    feed.scrollBy(1300);
    expect(scroller.isFollowing).toBe(false);
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

  describe("how near the end counts as back at the end", () => {
    it("measures from the end of the content, not from the end of the room reserved below it", () => {
      const { feed, scroller } = attached();
      // 300px of room under the last line: the content ends at 1700.
      feed.geometry.padding = 300;
      feed.scrollBy(0);
      expect(scroller.isFollowing).toBe(false);

      // Coming back down to 121px short of the last line is not yet the end.
      feed.scrollBy(1079);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.isFollowing).toBe(false);

      // 400px from the true bottom, but the last line is 100px under the view's foot.
      feed.scrollBy(1100);
      expect(scroller.scrolledUp).toBe(false);
      expect(scroller.isFollowing).toBe(true);
    });

    it("sees the last line only above what floats over the foot of the area", () => {
      let covered = 150;
      const { feed, scroller } = attached({ covered: () => covered });
      feed.geometry.padding = 150;
      // 50px short of the bottom, with the last line in view.
      feed.scrollBy(1450);
      expect(scroller.scrolledUp).toBe(false);

      // About the same position is worse with a taller composer: its 400px hide the last 300px of text.
      covered = 400;
      feed.scrollBy(1451);
      expect(scroller.scrolledUp).toBe(true);
    });

    it("keeps a reader at the very bottom following while the room for the composer catches up", () => {
      // The composer just grew to 137px; the room under the last line, set
      // from its height a frame later, is still the 48px buffer.
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

      // The reader scrolls up; before the scroll event arrives, a streamed
      // reply adds content and the page tells the scroller.
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
      // The feed pinned the reader with the composer's 109px already measured,
      // but the browser restyled the column with its room a frame later.
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
      // Seen on a phone while reconnecting: the composer covers 137px and the
      // room under the last line is still 48px.
      const { feed, observer, scroller } = attached({ covered: () => 137 });
      feed.geometry.padding = 48;
      feed.scrollTo.mockClear();
      // The feed pins the reader to the bottom.
      scroller.contentChanged(true);
      expect(feed.geometry.scrollTop).toBe(2000);
      // 46px more content lands, then the pin's own scroll event arrives,
      // with the position where the pin left it.
      feed.geometry.scrollHeight = 2546;
      feed.scrollBy(feed.geometry.scrollTop);
      expect(scroller.isFollowing).toBe(true);

      // Older history is prepended: the reader is kept at the newest.
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
      // The reserved room is the composer plus a 200px buffer, and the reader sits at the true bottom.
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

  describe("leaving the end", () => {
    it("leaves a reader who has started scrolling up where they are as a streamed reply lands", () => {
      const { feed, observer, scroller } = attached();
      // The reader, following at the bottom, drags 40px up, and the next piece of
      // the reply (25px) lands in the same frame.
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
      // Still near the end, so no pill yet.
      expect(scroller.scrolledUp).toBe(false);

      // A piece of the reply lands before the wheel's scroll does.
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
      // The browser keeps the position in range.
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
      // A swipe over a code block that is mostly sideways.
      feed.fire("wheel", { deltaY: -2, deltaX: 40 });
      feed.fire("wheel", { deltaY: -100, shiftKey: true });
      feed.fire("wheel", { deltaY: -100, ctrlKey: true });
      expect(scroller.isFollowing).toBe(true);
    });

    it("stops following at a key that scrolls up, and not at one something in the feed handled", () => {
      const { feed, scroller } = attached();
      feed.press({ key: "PageDown" });
      expect(scroller.isFollowing).toBe(true);
      feed.fire("keydown", { key: "ArrowUp", defaultPrevented: true });
      expect(scroller.isFollowing).toBe(true);
      feed.press({ key: "PageUp" });
      expect(scroller.isFollowing).toBe(false);
    });

    it("respects a move up with no input in the feed, as find in page or a screen reader makes", () => {
      const { feed, observer, scroller } = attached();
      // Find in page brings a match 60px up into view: no wheel, touch, key or pointer.
      feed.scrollBy(1440);
      expect(scroller.isFollowing).toBe(false);
      // Near enough the end that the pill would only be in the way.
      expect(scroller.scrolledUp).toBe(false);

      // A reply lands below them: they stay at the match, and the pill says so.
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
      // The items didn't change in a way the page reports; a frame goes by.
      observer.notify();
      feed.geometry.scrollTop = 1300;
      scroller.contentChanged();
      expect(scroller.isFollowing).toBe(false);
      expect(feed.geometry.scrollTop).toBe(1300);
    });

    it("doesn't take the browser keeping a reader at the bottom in range for them leaving", () => {
      const { feed, observer, scroller } = attached();
      // The view grows by 30px: the browser moves the position up to keep it in range.
      feed.geometry.clientHeight = 530;
      feed.scrollBy(1470);
      expect(scroller.isFollowing).toBe(true);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
    });
  });

  describe("a flick", () => {
    it("counts the momentum after a flick as the reader's", () => {
      const { feed, observer, scroller } = attached();
      feed.fire("touchstart");
      feed.scrollBy(1450);
      feed.fire("touchend", { touches: [] });
      expect(scroller.isFollowing).toBe(false);

      // The flick carries on with no finger on the feed, while the reply streams in.
      feed.scrollBy(1380);
      feed.geometry.scrollHeight = 2025;
      observer.notify();
      feed.scrollBy(1320);
      feed.geometry.scrollHeight = 2050;
      observer.notify();
      feed.fire("scrollend");

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
      feed.scrollTo.mockImplementationOnce(() => {});
      scroller.jumpToLatest();
      // The glide passes through on its way down.
      feed.scrollBy(600);
      feed.scrollBy(1200);
      expect(scroller.isFollowing).toBe(true);
      expect(scroller.scrolledUp).toBe(false);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
    });

    it("counts a reader who scrolls away after Jump to latest was cut short by new content", () => {
      const { feed, observer, scroller } = attached();
      // The reader is reading further up.
      feed.scrollBy(100);
      expect(scroller.scrolledUp).toBe(true);

      // They press Jump to latest; the browser's smooth glide hasn't moved yet.
      feed.scrollTo.mockImplementationOnce(() => {});
      scroller.jumpToLatest();
      // A streamed reply grows the feed before the glide's first frame: the feed pins
      // the reader to the bottom at once, which ends the glide, and the pin's own
      // scroll event finds the position where the pin left it.
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(2100);
      feed.scrollBy(feed.geometry.scrollTop);

      // The reader moves back up without a wheel, touch, key or pointer event
      // reaching the feed (a scrollbar drag, a screen reader, find in page).
      feed.scrollBy(0);
      expect(scroller.scrolledUp).toBe(true);
      expect(scroller.isFollowing).toBe(false);

      // More of the reply lands: the reader stays where they went.
      feed.geometry.scrollHeight = 2200;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(0);
    });

    it("counts a move up while it runs as the reader's", () => {
      const { feed, observer, scroller } = attached();
      feed.scrollBy(100);
      feed.scrollTo.mockImplementationOnce(() => {});
      scroller.jumpToLatest();
      feed.scrollBy(700);
      // The page scrolls back to the top mid-glide, with no input in the feed.
      feed.scrollBy(0);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(true);
      feed.geometry.scrollHeight = 2100;
      observer.notify();
      expect(feed.geometry.scrollTop).toBe(0);
    });

    it("ends at scrollend, after which a move up is the reader's", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      feed.scrollTo.mockImplementationOnce(() => {});
      scroller.jumpToLatest();
      feed.scrollBy(700);
      // Cut short somewhere on the way.
      feed.fire("scrollend");
      feed.scrollBy(650);
      expect(scroller.isFollowing).toBe(false);
      expect(scroller.scrolledUp).toBe(true);
    });

    it("ends at the reader's own input", () => {
      const { feed, scroller } = attached();
      feed.scrollBy(100);
      feed.scrollTo.mockImplementationOnce(() => {});
      scroller.jumpToLatest();
      feed.scrollBy(700);
      feed.fire("touchstart");
      feed.fire("touchend", { touches: [] });
      feed.scrollBy(650);
      expect(scroller.isFollowing).toBe(false);
    });
  });
});
