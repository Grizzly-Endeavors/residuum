import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FeedScroller } from "./feed-scroll.svelte";

// Opening and closing things in a feed, against real elements. jsdom lays
// nothing out, so each element's top is a number the test moves.

class ManualObserver {
  static instances: ManualObserver[] = [];
  constructor(private readonly callback: () => void) {
    ManualObserver.instances.push(this);
  }
  observe(): void {}
  disconnect(): void {}
  notify(): void {
    this.callback();
  }
}

interface Layout {
  tops: Map<Element, number>;
  scrollTop: number;
  scrollHeight: number;
  /** The content's height, which a hold keeps from shrinking. */
  contentHeight: number;
}

function place(el: Element, layout: Layout, top: number): void {
  layout.tops.set(el, top);
  el.getBoundingClientRect = () => ({ top: layout.tops.get(el) ?? 0 }) as DOMRect;
}

interface Feed {
  scroll: HTMLElement;
  content: HTMLElement;
  item: HTMLElement;
  button: HTMLButtonElement;
  layout: Layout;
  scroller: FeedScroller;
  scrollTo: ReturnType<typeof vi.fn>;
}

function feed(): Feed {
  vi.stubGlobal("ResizeObserver", ManualObserver);
  document.body.innerHTML = `
    <div id="scroll"><div id="content"><div data-feed-item id="item"><button aria-expanded="false" aria-controls="body">Show all</button></div></div></div>`;
  const scroll = document.getElementById("scroll") as HTMLElement;
  const content = document.getElementById("content") as HTMLElement;
  const item = document.getElementById("item") as HTMLElement;
  const button = item.querySelector("button") as HTMLButtonElement;
  const layout: Layout = {
    tops: new Map(),
    scrollTop: 1500,
    scrollHeight: 2000,
    contentHeight: 2000,
  };
  place(scroll, layout, 0);
  place(item, layout, 400);
  place(button, layout, 440);
  Object.defineProperty(scroll, "scrollHeight", { get: () => layout.scrollHeight });
  Object.defineProperty(scroll, "clientHeight", { get: () => 500 });
  Object.defineProperty(scroll, "scrollTop", { get: () => layout.scrollTop });
  Object.defineProperty(content, "offsetHeight", { get: () => layout.contentHeight });
  const scrollTo = vi.fn((options: { top: number }) => {
    layout.scrollTop = options.top;
  });
  scroll.scrollTo = scrollTo as unknown as typeof scroll.scrollTo;
  const scroller = new FeedScroller();
  scroller.attach(scroll, content);
  return { scroll, content, item, button, layout, scroller, scrollTo };
}

/** Callbacks waiting for the next frame, by the id `requestAnimationFrame` returned. */
const frameCallbacks = new Map<number, () => void>();
let nextFrameId = 1;

/** Run the frame: every callback requested before it runs once, and any it requests waits for the next. */
function runFrames(count: number): void {
  for (let n = 0; n < count; n++) {
    const due = [...frameCallbacks.values()];
    frameCallbacks.clear();
    for (const callback of due) callback();
  }
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", (callback: () => void): number => {
    const id = nextFrameId++;
    frameCallbacks.set(id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => {
    frameCallbacks.delete(id);
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  frameCallbacks.clear();
  ManualObserver.instances = [];
  document.body.innerHTML = "";
});

describe("FeedScroller, opening and closing things", () => {
  it("stops following when a control that opens something is pressed, so the new content doesn't carry it away", () => {
    const { button, scroller, layout, scrollTo } = feed();
    expect(scroller.isFollowing).toBe(true);

    button.click();
    expect(scroller.isFollowing).toBe(false);

    // What opened grows the content below the control: a following feed would
    // have pinned to the new bottom and taken the control with it.
    layout.scrollHeight = 2300;
    ManualObserver.instances[0]?.notify();
    expect(scrollTo).not.toHaveBeenCalled();
    expect(layout.scrollTop).toBe(1500);
  });

  it("puts the control back where it was when it is moved by what it opened", () => {
    const { button, layout, scrollTo } = feed();
    button.click();

    // The control's body grew above it, as "Show all" does: it is 120px lower.
    place(button, layout, 560);
    ManualObserver.instances[0]?.notify();
    expect(scrollTo).toHaveBeenLastCalledWith({ top: 1620, behavior: "instant" });
  });

  it("keeps the control still on the frames after, while what it opens settles", () => {
    const { button, layout, scrollTo } = feed();
    button.click();
    place(button, layout, 470);
    runFrames(1);
    expect(scrollTo).toHaveBeenLastCalledWith({ top: 1530, behavior: "instant" });
    // Back in place, so a later frame leaves it be.
    place(button, layout, 440);
    scrollTo.mockClear();
    runFrames(3);
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("finds the control again when what it opened redrew it", () => {
    const { button, item, layout, scrollTo } = feed();
    button.click();
    // The same control, drawn again as a new element.
    const again = document.createElement("button");
    again.setAttribute("aria-expanded", "true");
    again.setAttribute("aria-controls", "body");
    button.replaceWith(again);
    place(again, layout, 500);
    expect(item.contains(again)).toBe(true);
    ManualObserver.instances[0]?.notify();
    expect(scrollTo).toHaveBeenLastCalledWith({ top: 1560, behavior: "instant" });
  });

  it("holds the item in place while the control is away", () => {
    const { button, item, layout, scrollTo } = feed();
    button.click();
    button.remove();
    place(item, layout, 430);
    ManualObserver.instances[0]?.notify();
    expect(scrollTo).toHaveBeenLastCalledWith({ top: 1530, behavior: "instant" });
  });

  it("holds the content's height, so closing something at the end can't pull the thread down", () => {
    const { button, content } = feed();
    button.click();
    expect(content.style.minHeight).toBe("2000px");
  });

  it("lets go of the height, and the control, once the reader scrolls", () => {
    const { button, content, scroll, layout, scrollTo } = feed();
    button.click();
    scroll.dispatchEvent(new Event("wheel"));
    expect(content.style.minHeight).toBe("");

    scrollTo.mockClear();
    place(button, layout, 600);
    ManualObserver.instances[0]?.notify();
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("gives the place up after half a second", () => {
    const { button, layout, scrollTo } = feed();
    button.click();
    vi.advanceTimersByTime(520);
    scrollTo.mockClear();
    place(button, layout, 700);
    ManualObserver.instances[0]?.notify();
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("follows again once the reader is back at the end", () => {
    const { button, scroller, scroll, layout } = feed();
    button.click();
    expect(scroller.isFollowing).toBe(false);

    layout.scrollTop = 1500;
    scroll.dispatchEvent(new Event("scroll"));
    expect(scroller.isFollowing).toBe(true);
  });

  it("ignores a press on something that doesn't open or close anything", () => {
    const { scroller, content } = feed();
    const other = document.createElement("button");
    content.append(other);
    other.click();
    expect(scroller.isFollowing).toBe(true);
    expect(content.style.minHeight).toBe("");
  });
});
