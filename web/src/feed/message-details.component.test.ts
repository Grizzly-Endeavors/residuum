import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { advance, fireEvent, render, screen, settle } from "../test/component";
import { FeedStore } from "../lib/feed.svelte";
import { toast } from "../lib/toast.svelte";
import type { FeedItem } from "../lib/types";
import FeedItemView from "./FeedItemView.svelte";

// What a message keeps quiet until it is hovered, focused or tapped: when it
// was sent, and Copy on an agent's reply.

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** Show `item` the way the feed holds it, and give back that reactive item. */
function show(item: FeedItem, agent = "atlas"): FeedItem {
  const store = new FeedStore();
  store.feed.push(item);
  const held = store.feed[0];
  if (held === undefined) throw new Error("the feed didn't take the item");
  render(FeedItemView, { item: held, agent });
  return held;
}

function stubClipboard(writeText: (text: string) => Promise<void>): ReturnType<typeof vi.fn> {
  const spy = vi.fn(writeText);
  Object.defineProperty(navigator, "clipboard", { value: { writeText: spy }, configurable: true });
  return spy;
}

/** A screen where nothing hovers (`hover: none`), or one where something does. */
function stubHover(hover: boolean): void {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: query === "(hover: none)" ? !hover : false,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    })),
  );
}

/**
 * A user whose clicks advance the faked clock. It is made before the
 * clipboard is stubbed, since it installs a clipboard of its own.
 */
function pointer(): ReturnType<typeof userEvent.setup> {
  return userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
}

// Friday 9 October 2026, early evening, on the reader's clock.
const NOW = new Date(2026, 9, 9, 18, 0);

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", NoObserver);
  vi.useFakeTimers({
    toFake: ["Date", "setTimeout", "clearTimeout", "setInterval", "clearInterval"],
  });
  vi.setSystemTime(NOW);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("when a message was sent", () => {
  it("is a time element a screen reader reads, in the reader's locale", () => {
    show({ id: 1, kind: "user", content: "Hello", timestamp: "2026-10-09T10:05:00" });
    const time = screen.getByText(/10:05/);
    expect(time.tagName).toBe("TIME");
    expect(time).toHaveAttribute("datetime", new Date(2026, 9, 9, 10, 5).toISOString());
    expect(time.closest("[aria-hidden='true'], [hidden], [inert]")).toBeNull();
    expect(time).toHaveAttribute("title", expect.stringContaining("2026") as string);
  });

  it("names the date as well for a message from another day", () => {
    show({ id: 1, kind: "user", content: "Hello", timestamp: "2026-10-08T10:05:00" });
    expect(screen.getByText(/Oct 8/)).toHaveTextContent(/10:05/);
  });

  it("is on a reply as well as on the user's message", () => {
    show({ id: 1, kind: "assistant", content: "Done.", timestamp: "2026-10-09T11:00:00" });
    expect(screen.getByText(/11:00/).tagName).toBe("TIME");
  });

  it("waits with Copy while the reply streams in, out of reach, and is there once it is whole", async () => {
    const held = show({
      id: 1,
      kind: "assistant",
      content: "The port is",
      streaming: true,
      timestamp: "2026-10-09T11:00:00",
    });
    // jsdom has no inert, so the property Svelte sets is read as it is; a browser takes the
    // row out of the tab order and the accessibility tree.
    const foot = (): { inert?: boolean } | null =>
      document.querySelector<HTMLElement & { inert?: boolean }>(".reply-foot");
    expect(foot()?.inert).toBe(true);

    if (held.kind !== "assistant") throw new Error("not a reply");
    held.content = "The port is set twice.";
    held.streaming = false;
    await advance(0);

    expect(foot()?.inert).toBe(false);
    expect(screen.getByRole("button", { name: "Copy reply" })).toBeInTheDocument();
  });

  it("sits in the head of a message from a session", () => {
    show({
      id: 1,
      kind: "agent-message",
      from: "main",
      category: "main",
      content: "Result",
      runId: null,
      timestamp: "2026-10-09T12:30:00",
    });
    const head = screen.getByRole("article").querySelector("header");
    expect(head).toContainElement(screen.getByText(/12:30/));
  });

  it("sits beside the file it came with", () => {
    show({
      id: 1,
      kind: "file-attachment",
      filename: "report.pdf",
      mimeType: "application/pdf",
      size: 2048,
      url: "/files/report.pdf",
      caption: null,
      timestamp: "2026-10-09T13:15:00",
    });
    const link = screen.getByRole("link", { name: /report\.pdf/ });
    expect(link.parentElement).toContainElement(screen.getByText(/1:15/));
  });

  it("is left out where it isn't known, as for an archived message", () => {
    show({ id: 1, kind: "user", content: "Hello" });
    expect(document.querySelector("time")).toBeNull();
  });

  it("is left out when the timestamp names no moment", () => {
    show({ id: 1, kind: "user", content: "Hello", timestamp: "sometime" });
    expect(document.querySelector("time")).toBeNull();
  });

  it("names the date once midnight has passed", async () => {
    show({ id: 1, kind: "user", content: "Hello", timestamp: "2026-10-09T23:50:00" });
    const time = (): Element | null => document.querySelector("time");
    expect(time()).not.toHaveTextContent(/Oct/);

    vi.setSystemTime(new Date(2026, 9, 10, 0, 5));
    await advance(60_000);
    expect(time()).toHaveTextContent(/Oct 9/);
  });
});

describe("copying a reply", () => {
  it("puts the reply's Markdown source on the clipboard and says so politely", async () => {
    const user = pointer();
    const writeText = stubClipboard(() => Promise.resolve());
    show({ id: 1, kind: "assistant", content: "Use **two** `ports`.\n\n- a\n- b" });
    const copy = screen.getByRole("button", { name: "Copy reply" });
    expect(copy).toHaveTextContent("Copy");

    await user.click(copy);
    await settle();

    expect(writeText).toHaveBeenCalledWith("Use **two** `ports`.\n\n- a\n- b");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    // A button that is pressed doesn't announce its new label, so a status does.
    const status = (): Element | null => document.querySelector(".reply-foot [role=status]");
    expect(status()).toHaveTextContent("Copied");

    await advance(2000);
    expect(screen.getByRole("button", { name: "Copy reply" })).toBeInTheDocument();
    expect(status()?.textContent).toBe("");
  });

  it("is on every reply, including one from an archive with no time", () => {
    show({ id: 1, kind: "assistant", content: "Archived." });
    expect(screen.getByRole("button", { name: "Copy reply" })).toBeInTheDocument();
  });

  it("says when the reply couldn't be copied, in a toast", async () => {
    const user = pointer();
    stubClipboard(() => Promise.reject(new Error("denied")));
    const error = vi.spyOn(toast, "error");
    show({ id: 1, kind: "assistant", content: "Hello" });

    await user.click(screen.getByRole("button", { name: "Copy reply" }));
    await settle();

    expect(error).toHaveBeenCalledWith(expect.stringContaining("Couldn't copy the reply"));
    expect(screen.getByRole("button", { name: "Copy reply" })).toBeInTheDocument();
  });
});

describe("showing the quiet details on a tap", () => {
  const message = (): HTMLElement => {
    const el = document.querySelector<HTMLElement>(".feed-message");
    if (el === null) throw new Error("no message shown");
    return el;
  };

  it("toggles them where nothing hovers, from a tap on the text", async () => {
    const user = pointer();
    stubHover(false);
    show({ id: 1, kind: "assistant", content: "Plain words.", timestamp: "2026-10-09T10:05:00" });
    expect(message()).not.toHaveAttribute("data-revealed");

    await user.click(screen.getByText("Plain words."));
    expect(message()).toHaveAttribute("data-revealed");

    await user.click(screen.getByText("Plain words."));
    expect(message()).not.toHaveAttribute("data-revealed");
  });

  it("leaves a tap on a button or a link to that control", async () => {
    const user = pointer();
    stubHover(false);
    stubClipboard(() => Promise.resolve());
    show({
      id: 1,
      kind: "assistant",
      content: "See [the docs](https://example.com/docs).",
      timestamp: "2026-10-09T10:05:00",
    });
    // Following the link would navigate; the tap is what matters here.
    document.addEventListener("click", (event) => {
      event.preventDefault();
    });

    await user.click(screen.getByRole("button", { name: "Copy reply" }));
    await user.click(screen.getByRole("link", { name: "the docs" }));
    expect(message()).not.toHaveAttribute("data-revealed");
  });

  it("does nothing where a pointer can hover, which reveals them itself", async () => {
    const user = pointer();
    stubHover(true);
    show({ id: 1, kind: "assistant", content: "Plain words.", timestamp: "2026-10-09T10:05:00" });
    await user.click(screen.getByText("Plain words."));
    expect(message()).not.toHaveAttribute("data-revealed");
  });

  it("isn't a tap when it ends a text selection", () => {
    stubHover(false);
    show({ id: 1, kind: "assistant", content: "Plain words.", timestamp: "2026-10-09T10:05:00" });
    const text = screen.getByText("Plain words.");
    window.getSelection()?.selectAllChildren(text);

    // A drag that selects text ends in a click with the selection still there.
    void fireEvent.click(text);
    expect(message()).not.toHaveAttribute("data-revealed");
    window.getSelection()?.removeAllRanges();
  });
});
