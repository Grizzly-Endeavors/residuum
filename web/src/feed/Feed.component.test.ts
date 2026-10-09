import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, settle } from "../test/component";
import { htmlSnippet } from "../test/snippets";
import type { FeedItem } from "../lib/types";
import type { FeedHistory } from "./feed-history";
import Feed from "./Feed.svelte";

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
    expect(Array.from(block?.children ?? [], (el) => el.textContent.trim())).toEqual([
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
    expect(Array.from(items, (el) => el.textContent.trim())).toEqual([
      "Here is the plan.",
      "Thinking…",
    ]);
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
    await vi.waitFor(() => {
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
    await new Promise((resolve) => setTimeout(resolve, 20));
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
