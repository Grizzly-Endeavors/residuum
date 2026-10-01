/**
 * The one stack every overlay layer joins (design §1): the topmost layer
 * takes Esc, an outside pointer closes floating layers from the top down,
 * focus is trapped in the topmost modal and returns where it came from, and
 * behind a modal the page is inert and doesn't scroll.
 *
 * A layer's component owns its open state. The stack only asks it to close
 * (`dismiss`); the component releases its layer once it has.
 */

import { focusables, focusInitial, overlayHost } from "./host";

/**
 * A modal traps focus and makes everything below it inert. A float (menu,
 * popover, tooltip) does neither, and closes when a pointer goes down outside
 * it.
 */
export type LayerKind = "modal" | "float";

export interface LayerOptions {
  kind: LayerKind;
  /** The layer's root, inside the overlay host. */
  element: HTMLElement;
  /** Ask the layer's owner to close it: Esc, or for a float a pointer outside it. */
  dismiss: () => void;
  /** For a float: what opened it. A pointer on it doesn't count as outside, so the trigger can toggle. */
  anchor?: HTMLElement | null;
  /**
   * Where focus goes when the layer closes. The element focused when it
   * opened, by default. `false` for a layer that never takes focus (a
   * tooltip): closing it leaves focus where it is.
   */
  returnFocus?: HTMLElement | null | false;
}

export interface LayerHandle {
  /** Whether this layer is on top of every other open layer. */
  readonly topmost: boolean;
  /** Leave the stack. Focus returns to `returnFocus` if it was inside the layer or nowhere. */
  release: () => void;
}

interface Layer extends LayerOptions {
  returnTo: HTMLElement | null;
  /** The layer `returnTo` is in, if any: when both close at once, focus goes on to where that one returns. */
  returnLayer: Layer | undefined;
}

function focusedElement(): HTMLElement | null {
  const active = document.activeElement;
  return active instanceof HTMLElement && active !== document.body ? active : null;
}

class OverlayStack {
  /** Open layers, bottom to top. */
  private readonly layers: Layer[] = [];
  /** Elements this stack made inert, so it never clears inertness someone else set. */
  private readonly inerted = new Set<Element>();
  private savedScroll: { overflow: string; gutter: string } | null = null;
  private caught: Event | null = null;

  /** Join the stack. A new modal closes the floats that were open. */
  open(options: LayerOptions): LayerHandle {
    if (options.kind === "modal") {
      for (const layer of this.topFloats()) layer.dismiss();
    }
    const returnTo =
      options.returnFocus === false ? null : (options.returnFocus ?? focusedElement());
    const { layers } = this;
    const returnLayer = layers.find((open) => returnTo !== null && open.element.contains(returnTo));
    const layer: Layer = { ...options, returnTo, returnLayer };
    layers.push(layer);
    this.update();
    return {
      get topmost() {
        return layers.at(-1) === layer;
      },
      release: () => {
        this.release(layer);
      },
    };
  }

  /** Whether `event`, a pointer press, already closed a float; a scrim under it then stays put. */
  caughtBy(event: Event): boolean {
    return this.caught === event;
  }

  private release(layer: Layer): void {
    const at = this.layers.indexOf(layer);
    if (at < 0) return;
    this.layers.splice(at, 1);
    this.update();
    // Focus can't have been lost with a layer that never held it.
    if (layer.returnFocus === false) return;
    // Focus stays wherever the user moved it; only focus lost with the layer goes back.
    const focused = focusedElement();
    if (focused !== null && !layer.element.contains(focused)) return;
    let from = layer;
    let back = layer.returnTo;
    while (back !== null && !back.isConnected && from.returnLayer !== undefined) {
      from = from.returnLayer;
      back = from.returnTo;
    }
    if (back?.isConnected && back.closest("[inert]") === null) {
      back.focus();
      return;
    }
    const top = this.layers.at(-1);
    if (top !== undefined) focusInitial(top.element);
  }

  /** The floats above the topmost modal, topmost first. */
  private topFloats(): Layer[] {
    const floats: Layer[] = [];
    for (const layer of [...this.layers].reverse()) {
      if (layer.kind === "modal") break;
      floats.push(layer);
    }
    return floats;
  }

  private update(): void {
    let topModal = -1;
    this.layers.forEach((layer, i) => {
      if (layer.kind === "modal") topModal = i;
    });
    this.makeInert(topModal < 0 ? [] : this.coveredBy(topModal));
    this.lockScroll(topModal >= 0);
    const listening = this.layers.length > 0;
    for (const [type, listener, capture] of this.listeners) {
      if (listening) document.addEventListener(type, listener, capture);
      else document.removeEventListener(type, listener, capture);
    }
  }

  /** What the modal at `index` covers: the page outside the host, and the layers below it. */
  private coveredBy(index: number): Element[] {
    const host = overlayHost();
    const covered = [...document.body.children].filter((child) => child !== host);
    return covered.concat(this.layers.slice(0, index).map((layer) => layer.element));
  }

  private makeInert(elements: Element[]): void {
    const wanted = new Set(elements);
    for (const element of this.inerted) {
      if (wanted.has(element)) continue;
      element.removeAttribute("inert");
      this.inerted.delete(element);
    }
    for (const element of wanted) {
      if (element.hasAttribute("inert")) continue;
      element.setAttribute("inert", "");
      this.inerted.add(element);
    }
  }

  private lockScroll(locked: boolean): void {
    const root = document.documentElement;
    if (locked && this.savedScroll === null) {
      this.savedScroll = {
        overflow: root.style.overflow,
        gutter: root.style.getPropertyValue("scrollbar-gutter"),
      };
      // Keep the page from shifting sideways when its scrollbar goes away.
      if (window.innerWidth > root.clientWidth)
        root.style.setProperty("scrollbar-gutter", "stable");
      root.style.overflow = "hidden";
    } else if (!locked && this.savedScroll !== null) {
      root.style.overflow = this.savedScroll.overflow;
      root.style.setProperty("scrollbar-gutter", this.savedScroll.gutter);
      this.savedScroll = null;
    }
  }

  // Esc and Tab are heard as they bubble to the document, so a control
  // inside a layer that uses Esc itself can claim it with preventDefault.
  private readonly onKeydown = (event: KeyboardEvent): void => {
    const top = this.layers.at(-1);
    if (top === undefined || event.defaultPrevented || event.isComposing) return;
    if (event.key === "Escape") {
      // Closing the overlay is all Esc does: it doesn't also stop a reply.
      event.preventDefault();
      event.stopPropagation();
      top.dismiss();
    } else if (event.key === "Tab" && top.kind === "modal") {
      this.wrapFocus(top.element, event);
    }
  };

  /** Tab past either end of a modal comes round to the other end. */
  private wrapFocus(container: HTMLElement, event: KeyboardEvent): void {
    const items = focusables(container);
    const first = items[0];
    const last = items.at(-1);
    if (first === undefined || last === undefined) {
      event.preventDefault();
      return;
    }
    const active = focusedElement();
    const inside = active !== null && items.includes(active);
    const wrapTo = event.shiftKey ? last : first;
    if (!inside || active === (event.shiftKey ? first : last)) {
      event.preventDefault();
      wrapTo.focus();
    }
  }

  // Captured, so a float closes before whatever is under the pointer reacts.
  private readonly onPointerdown = (event: PointerEvent): void => {
    const target = event.target;
    if (!(target instanceof Node)) return;
    for (const layer of this.topFloats()) {
      if (layer.element.contains(target) || layer.anchor?.contains(target)) return;
      this.caught = event;
      layer.dismiss();
    }
  };

  private readonly listeners: readonly [string, EventListener, boolean][] = [
    ["keydown", this.onKeydown as EventListener, false],
    ["pointerdown", this.onPointerdown as EventListener, true],
  ];
}

/** The overlay stack. Layer components join it; nothing else needs to. */
export const stack = new OverlayStack();
