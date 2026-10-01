import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { keyboardScrollable } from "./keyboard-scrollable";

// jsdom lays nothing out, so each region states its own sizes, and frames
// run when the test says.

let frames: FrameRequestCallback[] = [];
let resized: (() => void) | null = null;

class FakeResizeObserver {
  constructor(callback: () => void) {
    resized = callback;
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {
    resized = null;
  }
}

function runFrames(): void {
  const due = frames;
  frames = [];
  for (const callback of due) callback(0);
}

/** A region whose content is `size.content` pixels tall in a box `size.box` tall. */
function region(size: { content: number; box: number }): HTMLElement {
  const element = document.createElement("div");
  Object.defineProperty(element, "clientHeight", { get: () => size.box });
  Object.defineProperty(element, "scrollHeight", { get: () => size.content });
  document.body.append(element);
  return element;
}

beforeEach(() => {
  frames = [];
  resized = null;
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
  vi.stubGlobal("cancelAnimationFrame", () => {});
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("keyboardScrollable", () => {
  it("is a tab stop while its content overflows, and not otherwise", () => {
    const size = { content: 100, box: 300 };
    const element = region(size);
    keyboardScrollable(element);
    expect(element).not.toHaveAttribute("tabindex");

    size.content = 900;
    resized?.();
    runFrames();
    expect(element).toHaveAttribute("tabindex", "0");

    size.content = 200;
    resized?.();
    runFrames();
    expect(element).not.toHaveAttribute("tabindex");
  });

  it("keeps its stop while it has focus, and gives it up when focus leaves", () => {
    const size = { content: 900, box: 300 };
    const element = region(size);
    keyboardScrollable(element);
    element.focus();
    expect(element).toHaveFocus();

    size.content = 200;
    resized?.();
    runFrames();
    expect(element).toHaveAttribute("tabindex", "0");
    expect(element).toHaveFocus();

    element.blur();
    runFrames();
    expect(element).not.toHaveAttribute("tabindex");
  });

  it("looks again when its content changes", async () => {
    const size = { content: 100, box: 300 };
    const element = region(size);
    keyboardScrollable(element);

    size.content = 900;
    element.append(document.createElement("p"));
    await vi.waitFor(() => {
      expect(frames).not.toHaveLength(0);
    });
    runFrames();
    expect(element).toHaveAttribute("tabindex", "0");
  });

  it("stops watching once it's detached", async () => {
    const size = { content: 100, box: 300 };
    const element = region(size);
    const detach = keyboardScrollable(element);
    if (typeof detach !== "function") throw new Error("the attachment returns its cleanup");
    detach();

    size.content = 900;
    element.append(document.createElement("p"));
    await Promise.resolve();
    expect(resized).toBeNull();
    expect(frames).toHaveLength(0);
    expect(element).not.toHaveAttribute("tabindex");
  });
});
