import { afterEach, describe, expect, it, vi } from "vitest";
import { FeedScroller } from "../lib/feed-scroll.svelte";
import type { FeedItem } from "../lib/types";
import { render, screen, settle } from "../test/component";
import { htmlSnippet } from "../test/snippets";
import Feed from "./Feed.svelte";

// The feed's scroller as the feed drives it, with a composer docked over its
// foot. jsdom lays nothing out, so the scrolling region and the dock get their
// sizes here.

/** A ResizeObserver that reports a resize only when the test says so. */
class CuedObserver {
  static instances: CuedObserver[] = [];
  readonly targets: Element[] = [];
  constructor(readonly callback: ResizeObserverCallback) {
    CuedObserver.instances.push(this);
  }
  observe(target: Element): void {
    this.targets.push(target);
  }
  unobserve(): void {}
  disconnect(): void {}
}

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const reply: FeedItem = { id: 2, kind: "assistant", content: "Here is the plan." };

afterEach(() => {
  CuedObserver.instances = [];
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the feed's scroller under a docked composer", () => {
  it("stays attached as the composer comes in and grows", async () => {
    vi.stubGlobal("ResizeObserver", CuedObserver);
    vi.stubGlobal("IntersectionObserver", NoObserver);
    // The reader is 500px above the end, where the scroller reads the dock's
    // height to judge where they are.
    const geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1000 };
    const composer = { height: 0 };
    const isRegion = (el: Element): boolean => el.classList.contains("feed-scroll");
    vi.spyOn(Element.prototype, "scrollHeight", "get").mockImplementation(function (this: Element) {
      return isRegion(this) ? geometry.scrollHeight : 0;
    });
    vi.spyOn(Element.prototype, "clientHeight", "get").mockImplementation(function (this: Element) {
      return isRegion(this) ? geometry.clientHeight : 0;
    });
    vi.spyOn(Element.prototype, "scrollTop", "get").mockImplementation(function (this: Element) {
      return isRegion(this) ? geometry.scrollTop : 0;
    });
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockImplementation(function (
      this: HTMLElement,
    ) {
      return this.classList.contains("feed-dock") ? composer.height : 0;
    });
    const attach = vi.spyOn(FeedScroller.prototype, "attach");

    const { container } = render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
      dock: htmlSnippet('<form aria-label="Composer"></form>'),
    });
    const region = screen.getByRole("region", { name: "Conversation with atlas" });
    region.scrollTo = ((options: ScrollToOptions) => {
      const top = typeof options.top === "number" ? options.top : 0;
      geometry.scrollTop = Math.min(top, geometry.scrollHeight - geometry.clientHeight);
    }) as typeof region.scrollTo;
    await settle();
    expect(attach).toHaveBeenCalledOnce();

    const dock = container.querySelector(".feed-dock");
    if (!dock) throw new Error("the dock didn't render");
    for (const height of [60, 160, 90]) {
      composer.height = height;
      for (const observer of CuedObserver.instances) {
        if (observer.targets.includes(dock)) {
          observer.callback([], observer as unknown as ResizeObserver);
        }
      }
      await settle();
    }
    expect(attach).toHaveBeenCalledOnce();
  });
});
