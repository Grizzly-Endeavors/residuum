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
  geometry: { scrollHeight: number; clientHeight: number; scrollTop: number };
}

function fakeScroller(): FakeScroller {
  const geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1500 };
  const listeners = new Map<string, () => void>();
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
    addEventListener: (type: string, listener: () => void) => listeners.set(type, listener),
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
