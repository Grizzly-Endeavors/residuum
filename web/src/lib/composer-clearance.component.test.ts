import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { composerClearance } from "./composer-clearance.svelte";

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

const VIEWPORT = 900;

/** An element whose top the test moves. */
function composerAt(top: number): { el: HTMLElement; moveTo: (top: number) => void } {
  let current = top;
  const el = document.createElement("form");
  document.body.append(el);
  el.getBoundingClientRect = () => ({ top: current }) as DOMRect;
  el.getClientRects = () => [{}] as unknown as DOMRectList;
  return {
    el,
    moveTo: (next) => {
      current = next;
    },
  };
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", ManualObserver);
  Object.defineProperty(document.documentElement, "clientHeight", {
    configurable: true,
    get: () => VIEWPORT,
  });
});

afterEach(() => {
  ManualObserver.instances = [];
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("composerClearance", () => {
  it("is nothing while no composer is up", () => {
    expect(composerClearance.px).toBe(0);
  });

  it("is the distance from the foot of the viewport to the composer's top, while it is mounted", () => {
    const { el } = composerAt(790);
    const release = composerClearance.track(el);
    expect(composerClearance.px).toBe(110);
    release();
    expect(composerClearance.px).toBe(0);
  });

  it("follows the composer growing, and the window changing size", () => {
    const { el, moveTo } = composerAt(790);
    const release = composerClearance.track(el);

    // Typing grew it: its top is higher up.
    moveTo(700);
    ManualObserver.instances[0]?.notify();
    expect(composerClearance.px).toBe(200);

    moveTo(650);
    window.dispatchEvent(new Event("resize"));
    expect(composerClearance.px).toBe(250);
    release();
  });

  it("goes to the composer mounted last, and back when it leaves", () => {
    const chat = composerAt(790);
    const session = composerAt(820);
    const releaseChat = composerClearance.track(chat.el);
    const releaseSession = composerClearance.track(session.el);
    expect(composerClearance.px).toBe(80);

    releaseSession();
    expect(composerClearance.px).toBe(110);
    releaseChat();
  });

  it("passes over a composer that isn't laid out", () => {
    const chat = composerAt(790);
    const hidden = composerAt(0);
    hidden.el.getClientRects = () => [] as unknown as DOMRectList;
    const releaseChat = composerClearance.track(chat.el);
    const releaseHidden = composerClearance.track(hidden.el);
    expect(composerClearance.px).toBe(110);
    releaseHidden();
    releaseChat();
  });
});
