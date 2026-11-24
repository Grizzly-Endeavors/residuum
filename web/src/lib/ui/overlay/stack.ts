import { focusables, focusInitial, overlayHost } from "./host";

export type LayerKind = "modal" | "float";

export interface LayerOptions {
  kind: LayerKind;
  element: HTMLElement;
  dismiss: () => void;
  anchor?: HTMLElement | null;
  returnFocus?: HTMLElement | null | false;
  spare?: Element | null;
}

export interface LayerHandle {
  readonly topmost: boolean;
  release: () => void;
}

interface Layer extends LayerOptions {
  returnTo: HTMLElement | null;
  returnLayer: Layer | undefined;
}

function coverAround(root: Element, spared: Element | null): Element[] {
  if (spared === null || !root.contains(spared)) return [root];
  if (root === spared) return [];
  return [...root.children].flatMap((child) => coverAround(child, spared));
}

function focusedElement(): HTMLElement | null {
  const active = document.activeElement;
  return active instanceof HTMLElement && active !== document.body ? active : null;
}

class OverlayStack {
  private readonly layers: Layer[] = [];
  private readonly inerted = new Set<Element>();
  private savedScroll: { overflow: string; gutter: string } | null = null;
  private caught: Event | null = null;

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

  get anyOpen(): boolean {
    return this.layers.length > 0;
  }

  caughtBy(event: Event): boolean {
    return this.caught === event;
  }

  private release(layer: Layer): void {
    const at = this.layers.indexOf(layer);
    if (at < 0) return;
    this.layers.splice(at, 1);
    this.update();
    if (layer.returnFocus === false) return;
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
    const spared = this.layers[index]?.spare ?? null;
    const covered = [...document.body.children]
      .filter((child) => child !== host)
      .flatMap((child) => coverAround(child, spared));
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
      if (window.innerWidth > root.clientWidth)
        root.style.setProperty("scrollbar-gutter", "stable");
      root.style.overflow = "hidden";
    } else if (!locked && this.savedScroll !== null) {
      root.style.overflow = this.savedScroll.overflow;
      root.style.setProperty("scrollbar-gutter", this.savedScroll.gutter);
      this.savedScroll = null;
    }
  }

  private readonly onKeydown = (event: KeyboardEvent): void => {
    const top = this.layers.at(-1);
    if (top === undefined || event.defaultPrevented || event.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      top.dismiss();
    } else if (event.key === "Tab") {
      const modal = this.trappingModal();
      if (modal !== undefined) this.wrapFocus(modal.element, event);
    }
  };

  private trappingModal(): Layer | undefined {
    const active = focusedElement();
    const holdsFocus = this.topFloats().some(
      (layer) => active !== null && layer.element.contains(active),
    );
    if (holdsFocus) return undefined;
    return this.layers.findLast((layer) => layer.kind === "modal");
  }

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

export const stack = new OverlayStack();

export function overlayOpen(): boolean {
  return stack.anyOpen;
}
