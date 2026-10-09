import { act, cleanup } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "../test/component";
import { FeedStore } from "../lib/feed.svelte";
import { router } from "../lib/router.svelte";
import type { FeedItem } from "../lib/types";
import FeedItemView from "./FeedItemView.svelte";

// A picture from the conversation, opened full size in a modal layer.

class NoObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

function show(item: FeedItem): void {
  const store = new FeedStore();
  store.feed.push(item);
  const held = store.feed[0];
  if (held === undefined) throw new Error("the feed didn't take the item");
  render(FeedItemView, { item: held, agent: "atlas" });
}

const THREE = [
  { media_type: "image/png", data: "AAAA" },
  { media_type: "image/jpeg", data: "BBBB" },
  { media_type: "image/gif", data: "CCCC" },
];

const opener = (n: number): HTMLElement =>
  screen.getByRole("button", { name: `View full size: Attached image ${String(n)}` });

/** The picture the viewer shows. */
const shown = (): HTMLElement => screen.getByRole("dialog").querySelector("img") as HTMLElement;

beforeAll(() => {
  router.startForOverlays();
});

afterAll(() => {
  router.stop();
});

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", NoObserver);
});

// Unmounting closes what a test left open, and that goes back in the history:
// let it land before the next test opens anything.
afterEach(async () => {
  cleanup();
  await vi.waitFor(() => {
    expect((window.history.state as { overlay?: string } | null)?.overlay).toBeUndefined();
  });
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
});

describe("opening a message's images full size", () => {
  it("opens the picture that was pressed, named by its words, in a modal", async () => {
    const user = userEvent.setup();
    show({ id: 1, kind: "user", content: "Look", images: THREE });
    await user.click(opener(2));

    const dialog = screen.getByRole("dialog", { name: "Attached image 2" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog.closest("[data-frame]")).toHaveAttribute("data-frame", "viewer");
    expect(shown()).toHaveAttribute("src", "data:image/jpeg;base64,BBBB");
    expect(shown()).toHaveAttribute("alt", "Attached image 2");
  });

  it("closes on Esc and the close button, and puts focus back on the picture", async () => {
    const user = userEvent.setup();
    show({ id: 1, kind: "user", content: "Look", images: THREE });
    await user.click(opener(1));
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(opener(1)).toHaveFocus();

    await user.click(opener(1));
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(opener(1)).toHaveFocus();
  });

  it("moves between the message's images with the arrow keys, wrapping at the ends", async () => {
    const user = userEvent.setup();
    show({ id: 1, kind: "user", content: "Look", images: THREE });
    await user.click(opener(1));

    await user.keyboard("{ArrowRight}");
    expect(shown()).toHaveAttribute("alt", "Attached image 2");
    await user.keyboard("{ArrowRight}");
    expect(shown()).toHaveAttribute("alt", "Attached image 3");
    await user.keyboard("{ArrowRight}");
    expect(shown()).toHaveAttribute("alt", "Attached image 1");
    await user.keyboard("{ArrowLeft}");
    expect(shown()).toHaveAttribute("alt", "Attached image 3");
    expect(screen.getByRole("dialog", { name: "Attached image 3" })).toBeInTheDocument();
  });

  it("says which picture it is on, and has buttons for those without arrow keys", async () => {
    const user = userEvent.setup();
    show({ id: 1, kind: "user", content: "Look", images: THREE });
    await user.click(opener(1));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("1 of 3");

    await user.click(screen.getByRole("button", { name: "Next image" }));
    expect(dialog).toHaveTextContent("2 of 3");
    expect(screen.getByRole("status")).toHaveTextContent("2 of 3: Attached image 2");

    await user.click(screen.getByRole("button", { name: "Previous image" }));
    await user.click(screen.getByRole("button", { name: "Previous image" }));
    expect(shown()).toHaveAttribute("alt", "Attached image 3");
  });

  it("has nothing to move between, and ignores the arrow keys, for a single image", async () => {
    const user = userEvent.setup();
    show({ id: 1, kind: "user", content: "Look", images: [THREE[0] as (typeof THREE)[number]] });
    await user.click(opener(1));

    expect(screen.queryByRole("button", { name: "Next image" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Previous image" })).toBeNull();
    await user.keyboard("{ArrowRight}");
    expect(shown()).toHaveAttribute("alt", "Attached image 1");
  });

  it("leaves a message with no images without a viewer", () => {
    show({ id: 1, kind: "user", content: "Just words" });
    expect(screen.queryByRole("button", { name: /View full size/ })).toBeNull();
  });
});

describe("opening an image the agent sent full size", () => {
  const sent = {
    id: 1,
    kind: "file-attachment",
    filename: "usage.png",
    mimeType: "image/png",
    size: 4096,
    url: "/files/usage.png",
    caption: "This week's usage",
  } as const;

  it("shows it with its caption as its words, and the file to download stays", async () => {
    const user = userEvent.setup();
    show(sent);
    await user.click(screen.getByRole("button", { name: "View full size: This week's usage" }));

    const dialog = screen.getByRole("dialog", { name: "This week's usage" });
    expect(shown()).toHaveAttribute("src", "/files/usage.png");
    expect(dialog).not.toHaveTextContent("1 of 1");
    expect(screen.queryByRole("button", { name: "Next image" })).toBeNull();

    await user.keyboard("{Escape}");
    expect(screen.getByRole("link", { name: /usage\.png/ })).toHaveAttribute(
      "href",
      "/files/usage.png",
    );
  });

  it("falls back to the file name for an image with no caption", async () => {
    const user = userEvent.setup();
    show({ ...sent, caption: null });
    await user.click(screen.getByRole("button", { name: "View full size: usage.png" }));
    expect(screen.getByRole("dialog", { name: "usage.png" })).toBeInTheDocument();
  });

  it("has no viewer for a file that isn't an image", () => {
    show({ ...sent, filename: "report.pdf", mimeType: "application/pdf", caption: null });
    expect(screen.queryByRole("button", { name: /View full size/ })).toBeNull();
  });
});
