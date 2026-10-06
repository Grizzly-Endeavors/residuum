import { afterEach, describe, expect, it, vi } from "vitest";
import { FeedScroller } from "./feed-scroll.svelte";

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

interface FakeScroller {
  el: HTMLElement;
  content: HTMLElement;
  scrollTo: ReturnType<typeof vi.fn>;
  /** Move the scroll position the way the reader does, and tell the scroller. */
  scrollBy: (top: number) => void;
  /** Press a key in the feed. */
  press: (init: KeyboardEventInit) => void;
  geometry: { scrollHeight: number; clientHeight: number; scrollTop: number };
}

function fakeScroller(): FakeScroller {
  const geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1500 };
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
  function attached(): { feed: FakeScroller; observer: ManualObserver } {
    vi.stubGlobal("ResizeObserver", ManualObserver);
    const feed = fakeScroller();
    new FeedScroller().attach(feed.el, feed.content);
    const observer = ManualObserver.instances[0];
    if (observer === undefined) throw new Error("the scroller made no ResizeObserver");
    return { feed, observer };
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
    // The composer below the feed grew, so the same content has less room.
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
});
