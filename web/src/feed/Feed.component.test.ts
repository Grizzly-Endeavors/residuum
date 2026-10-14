import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, settle } from "../test/component";
import { htmlSnippet } from "../test/snippets";
import { FeedScroller } from "../lib/feed-scroll.svelte";
import type { FeedItem } from "../lib/types";
import type { FeedHistory } from "./feed-history";
import Feed from "./Feed.svelte";
import { waitFor } from "../test/wait";

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** An observer that reports its target in view as soon as it watches it, like a top that's on screen. */
class InViewObserver {
  constructor(private readonly callback: (entries: { isIntersecting: boolean }[]) => void) {}
  observe(): void {
    this.callback([{ isIntersecting: true }]);
  }
  unobserve(): void {}
  disconnect(): void {}
}

/** What an element says, less the quiet details of its messages (when they were sent, Copy). */
function said(element: Element): string {
  const copy = element.cloneNode(true) as Element;
  for (const meta of copy.querySelectorAll("[data-message-meta]")) meta.remove();
  return copy.textContent.trim();
}

/** History with `parts` older parts, each load taking a moment. */
function olderHistory(parts: number): {
  history: FeedHistory;
  loadOlder: ReturnType<typeof vi.fn>;
} {
  const state = { remaining: parts, loading: false };
  const loadOlder = vi.fn(async () => {
    state.loading = true;
    await Promise.resolve();
    state.remaining--;
    state.loading = false;
    return true;
  });
  return {
    history: {
      get hasMore() {
        return state.remaining > 0;
      },
      get loadingOlder() {
        return state.loading;
      },
      generation: 0,
      loadOlder,
    },
    loadOlder,
  };
}

const reply: FeedItem = { id: 2, kind: "assistant", content: "Here is the plan." };
const tools: FeedItem = {
  id: 3,
  kind: "tool-group",
  calls: [{ id: "c1", name: "memory_search", arguments: {}, status: "done" }],
};

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", NoObserver);
  vi.stubGlobal("IntersectionObserver", NoObserver);
});

describe("Feed", () => {
  it("shows its one empty state when there's nothing to show", () => {
    render(Feed, {
      agent: "atlas",
      items: [],
      label: "Conversation with atlas",
      empty: htmlSnippet("<p>No messages yet</p>"),
    });
    expect(screen.getByRole("region", { name: "Conversation with atlas" })).toBeInTheDocument();
    expect(screen.getAllByText("No messages yet")).toHaveLength(1);
  });

  it("waits for the conversation before calling it empty", () => {
    render(Feed, {
      agent: "atlas",
      items: [],
      label: "Conversation with atlas",
      loading: true,
      empty: htmlSnippet("<p>No messages yet</p>"),
    });
    expect(screen.queryByText("No messages yet")).toBeNull();
  });

  it("keeps a turn's activity line where it happened, between what the agent said", () => {
    const { container } = render(Feed, {
      agent: "atlas",
      items: [
        { id: 10, kind: "user", content: "Plan the week", turnId: "t1" },
        { id: 11, kind: "assistant", content: "Checking first.", turnId: "t1" },
        { ...tools, id: 12, turnId: "t1" },
        { id: 13, kind: "assistant", content: "Here is the plan.", turnId: "t1" },
      ],
      label: "Conversation with atlas",
    });
    const kinds = Array.from(container.querySelectorAll<HTMLElement>("[data-feed-item]"), (el) => [
      el.dataset.kind,
      el.parentElement?.classList.contains("feed-turn") ?? false,
    ]);
    expect(kinds).toEqual([
      ["user", false],
      ["assistant", true],
      ["assistant", true],
    ]);
    const block = container.querySelector(".feed-turn");
    expect(Array.from(block?.children ?? [], said)).toEqual([
      "Checking first.",
      "Searched memory",
      "Here is the plan.",
    ]);
  });

  it("tells a screen reader what the agent is doing, from outside the scrolling region", async () => {
    const view = render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
      announcement: { id: 1, text: "atlas is working" },
    });
    const status = screen.getByText("atlas is working").closest('[role="status"]');
    expect(status).not.toBeNull();
    expect(screen.getByRole("region", { name: "Conversation with atlas" })).not.toContainElement(
      status as HTMLElement,
    );

    await view.rerender({ announcement: { id: 2, text: "atlas replied: Done." } });
    expect(status).toHaveTextContent(/^atlas replied: Done\.$/);
  });

  it("says nothing until there is something to say", () => {
    render(Feed, { agent: "atlas", items: [reply], label: "Conversation with atlas" });
    expect(screen.queryByText(/is working|replied|couldn't finish/)).toBeNull();
  });

  it("shows a turn that has only made tool calls as its line", () => {
    render(Feed, {
      agent: "atlas",
      items: [{ ...tools, turnId: "t1" }],
      label: "Conversation with atlas",
    });
    expect(screen.getByRole("button", { name: "Searched memory" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("shows the turn in flight from its start, with Stop, before it has any output", () => {
    const onStop = vi.fn();
    render(Feed, {
      agent: "atlas",
      items: [{ id: 10, kind: "user", content: "Plan the week", turnId: "t1" }],
      label: "Conversation with atlas",
      liveTurnId: "t1",
      onStop,
    });
    expect(screen.getByText("Working")).toBeInTheDocument();
    screen.getByRole("button", { name: "Stop the reply" }).click();
    expect(onStop).toHaveBeenCalledOnce();
  });

  it("puts the live tail after the items", () => {
    const { container } = render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
      tail: htmlSnippet('<p data-testid="live">Thinking…</p>'),
    });
    const items = container.querySelectorAll("[data-feed-item], [data-testid=live]");
    expect(Array.from(items, said)).toEqual(["Here is the plan.", "Thinking…"]);
  });

  it("keeps loading older parts while the top stays in view, until there are none", async () => {
    vi.stubGlobal("IntersectionObserver", InViewObserver);
    const { history, loadOlder } = olderHistory(3);
    render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
      history,
    });
    await waitFor(() => {
      expect(loadOlder).toHaveBeenCalledTimes(3);
    });
    expect(history.hasMore).toBe(false);
  });

  it("stops loading after a part fails to load", async () => {
    vi.stubGlobal("IntersectionObserver", InViewObserver);
    const loadOlder = vi.fn(() => Promise.resolve(false));
    render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
      history: { hasMore: true, loadingOlder: false, generation: 0, loadOlder },
    });
    await waitFor(() => {
      expect(loadOlder).toHaveBeenCalled();
    });
    await settle();
    // The observer and the history's change each try once; a failure doesn't chain into more.
    expect(loadOlder.mock.calls.length).toBeGreaterThan(0);
    expect(loadOlder.mock.calls.length).toBeLessThanOrEqual(2);
  });
});

/** Gives the scrolling region a size and position, which jsdom doesn't lay out. */
function sizeRegion(
  region: HTMLElement,
  geometry: { scrollHeight: number; clientHeight: number; scrollTop: number },
): ReturnType<typeof vi.fn> {
  Object.defineProperty(region, "scrollHeight", { get: () => geometry.scrollHeight });
  Object.defineProperty(region, "clientHeight", { get: () => geometry.clientHeight });
  Object.defineProperty(region, "scrollTop", { get: () => geometry.scrollTop });
  const scrollTo = vi.fn((options: { top: number }) => {
    geometry.scrollTop = options.top;
  });
  region.scrollTo = scrollTo as unknown as typeof region.scrollTo;
  return scrollTo;
}

function scrollReader(region: HTMLElement, geometry: { scrollTop: number }, top: number): void {
  geometry.scrollTop = top;
  region.dispatchEvent(new Event("scroll"));
}

describe("Jump to latest", () => {
  const items: FeedItem[] = [
    { id: 1, kind: "user", content: "First" },
    { id: 2, kind: "assistant", content: "Second" },
  ];

  async function reading(): Promise<{
    region: HTMLElement;
    geometry: { scrollHeight: number; clientHeight: number; scrollTop: number };
    scrolled: ReturnType<typeof vi.fn>;
    rerender: (props: { items: FeedItem[] }) => Promise<void>;
  }> {
    const { rerender } = render(Feed, {
      agent: "atlas",
      items,
      label: "Conversation with atlas",
    });
    const region = screen.getByRole("region", { name: "Conversation with atlas" });
    const geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1500 };
    const scrolled = sizeRegion(region, geometry);
    await settle();
    return { region, geometry, scrolled, rerender: (props) => rerender(props) };
  }

  it("shows nothing while the reader is at the end, and a pill once they scroll away", async () => {
    const { region, geometry } = await reading();
    scrollReader(region, geometry, 1500);
    await settle();
    expect(screen.queryByRole("button", { name: /Jump to latest/ })).toBeNull();

    scrollReader(region, geometry, 100);
    await settle();
    const pill = screen.getByRole("button", { name: "Jump to latest" });
    // An arrow pointing down, and no reading of where the reader is.
    expect(pill.querySelector("svg")).not.toBeNull();
    expect(pill).not.toHaveAttribute("aria-describedby");
    expect(pill.closest("[role=status]")).not.toBeNull();
  });

  it("says when a new item has landed below the reader, and clears when they are back", async () => {
    const { region, geometry, rerender } = await reading();
    scrollReader(region, geometry, 100);
    await settle();
    expect(screen.getByRole("button", { name: "Jump to latest" })).toHaveTextContent(
      "Jump to latest",
    );

    await rerender({ items: [...items, { id: 3, kind: "assistant", content: "Third" }] });
    await settle();
    const fresh = screen.getByRole("button", { name: "New reply, jump to latest" });
    expect(fresh).toHaveTextContent("New reply");
    expect(fresh).toHaveAttribute("data-new");

    scrollReader(region, geometry, 1500);
    await settle();
    expect(screen.queryByRole("button", { name: /Jump to latest|New reply/ })).toBeNull();
  });

  it("takes the reader to the newest content and drops the pill", async () => {
    const { region, geometry, scrolled } = await reading();
    scrollReader(region, geometry, 100);
    await settle();
    screen.getByRole("button", { name: "Jump to latest" }).click();
    await settle();
    expect(scrolled).toHaveBeenCalledWith({ top: 2000, behavior: "smooth" });
    expect(screen.queryByRole("button", { name: /Jump to latest/ })).toBeNull();
  });

  it("jumps without gliding when the reader asks for less motion", async () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query === "(prefers-reduced-motion: reduce)",
    }));
    const { region, geometry, scrolled } = await reading();
    scrollReader(region, geometry, 100);
    await settle();
    screen.getByRole("button", { name: "Jump to latest" }).click();
    expect(scrolled).toHaveBeenCalledWith({ top: 2000, behavior: "instant" });
  });
});

describe("following", () => {
  it("keeps the reader at the end when the browser moved them just before older items landed", async () => {
    const recent: FeedItem[] = [
      { id: 10, kind: "user", content: "Recent question" },
      { id: 11, kind: "assistant", content: "Recent answer" },
    ];
    const older: FeedItem[] = [
      { id: 1, kind: "user", content: "Older question" },
      { id: 2, kind: "assistant", content: "Older answer" },
    ];
    const { rerender } = render(Feed, {
      agent: "atlas",
      items: recent,
      label: "Conversation with atlas",
    });
    const region = screen.getByRole("region", { name: "Conversation with atlas" });
    const geometry = { clientHeight: 500, scrollTop: 1500 };
    // Each item adds 430px once it is in the page: 2000px with the recent two.
    Object.defineProperty(region, "scrollHeight", {
      get: () => 1140 + 430 * region.querySelectorAll("[data-feed-item]").length,
    });
    Object.defineProperty(region, "clientHeight", { get: () => geometry.clientHeight });
    Object.defineProperty(region, "scrollTop", { get: () => geometry.scrollTop });
    region.scrollTo = ((options: { top: number }) => {
      geometry.scrollTop = options.top;
    }) as typeof region.scrollTo;
    await settle();
    scrollReader(region, geometry, 1500);
    await settle();

    // Seen on a 320px phone as the chat opened: the page settled around the
    // feed, so the browser moved the position back 4px to keep it in range,
    // and an older episode was prepended before that move's scroll event.
    geometry.clientHeight = 504;
    geometry.scrollTop = 1496;
    await rerender({ items: [...older, ...recent] });
    await settle();
    region.dispatchEvent(new Event("scroll"));
    await settle();

    expect(screen.queryByRole("button", { name: /Jump to latest/ })).toBeNull();
    expect(geometry.scrollTop).toBe(2860);
  });
});

describe("a dock over the foot of the feed", () => {
  it("floats beside the scrolling region, outside it, and tells the feed it is docked", () => {
    const { container } = render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
      dock: htmlSnippet('<form aria-label="Composer"></form>'),
    });
    const feed = container.querySelector(".feed");
    expect(feed).toHaveAttribute("data-docked");
    const region = screen.getByRole("region", { name: "Conversation with atlas" });
    const composer = screen.getByRole("form", { name: "Composer" });
    expect(region.contains(composer)).toBe(false);
    expect(composer.closest(".feed-dock")?.parentElement).toBe(feed);
  });

  it("is plain without one", () => {
    const { container } = render(Feed, {
      agent: "atlas",
      items: [reply],
      label: "Conversation with atlas",
    });
    expect(container.querySelector(".feed")).not.toHaveAttribute("data-docked");
    expect(container.querySelector(".feed-dock")).toBeNull();
  });
});

/** A ResizeObserver that reports a resize only when the test says so, so a dock can be resized on cue. */
class CuedObserver {
  readonly targets: Element[] = [];
  constructor(readonly callback: ResizeObserverCallback) {}
  observe(target: Element): void {
    this.targets.push(target);
  }
  unobserve(): void {}
  disconnect(): void {}
}

describe("the scroller under a dock that changes size", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows no pill to a reader near the end as the composer grows, and never re-attaches the scroller", async () => {
    const observers: CuedObserver[] = [];
    vi.stubGlobal(
      "ResizeObserver",
      class extends CuedObserver {
        constructor(callback: ResizeObserverCallback) {
          super(callback);
          observers.push(this);
        }
      },
    );
    // jsdom lays nothing out, so the region and the dock get their sizes here.
    // The region is 50px above the end from the start, which is where the
    // scroller reads the dock's height while it attaches.
    const geometry = { scrollHeight: 2000, clientHeight: 500, scrollTop: 1450 };
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
    const scrolled = vi.fn((options: ScrollToOptions) => {
      const top = typeof options.top === "number" ? options.top : 0;
      geometry.scrollTop = Math.min(top, geometry.scrollHeight - geometry.clientHeight);
    });
    region.scrollTo = scrolled as unknown as typeof region.scrollTo;
    const dock = container.querySelector(".feed-dock");
    if (!dock) throw new Error("the dock didn't render");
    const resizeDock = async (height: number): Promise<void> => {
      composer.height = height;
      for (const observer of observers) {
        if (observer.targets.includes(dock)) {
          observer.callback([], observer as unknown as ResizeObserver);
        }
      }
      await settle();
    };

    // The composer comes up under a reader at the end, which keeps them there.
    await resizeDock(60);
    expect(geometry.scrollTop).toBe(1500);

    // The reader scrolls 50px above the end: they stop following, near enough the end that no pill shows.
    scrollReader(region, geometry, 1450);
    await settle();
    expect(screen.queryByRole("button", { name: /Jump to latest/ })).toBeNull();

    // The composer grows by 100px. The scroller stays attached: it isn't measured afresh.
    await resizeDock(160);
    expect(screen.queryByRole("button", { name: /Jump to latest/ })).toBeNull();
    expect(attach).toHaveBeenCalledOnce();
  });
});
