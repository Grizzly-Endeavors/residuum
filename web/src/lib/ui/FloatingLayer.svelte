<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import type { HTMLAttributes } from "svelte/elements";
  import { focusables, focusInitial, portal } from "./overlay/host";
  import { placeFloat } from "./overlay/placement";
  import { stack } from "./overlay/stack";
  import type { FloatAlign, FloatShape, FloatSide } from "./types";

  // The base menus, popovers and tooltips are drawn on: a card in the overlay
  // host, beside its anchor and inside the viewport, that joins the overlay
  // stack as a float. It is controlled: it asks to close through `onclose`,
  // and closes when the owner sets `open` false.

  interface Props extends Omit<HTMLAttributes<HTMLDivElement>, "children" | "style"> {
    open: boolean;
    /** What the layer sits beside. A pointer on it doesn't count as outside, and focus returns to it. */
    anchor: HTMLElement | null | undefined;
    shape: FloatShape;
    side?: FloatSide;
    align?: FloatAlign;
    /** The card's width, for a popover. */
    width?: string;
    /**
     * What takes focus on open: a selector's match (`[data-autofocus]` by
     * default, else the first control, else the card), a function that moves
     * focus itself, or false to leave focus where it is, as a tooltip does.
     */
    initialFocus?: string | false | ((layer: HTMLElement) => void);
    /** The user asked to close it: Esc, a pointer outside, or Tab out of it. */
    onclose: () => void;
    children: Snippet;
  }

  let {
    open,
    anchor,
    shape,
    side = "bottom",
    align = "start",
    width,
    initialFocus,
    onclose,
    class: className,
    children,
    ...rest
  }: Props = $props();

  /** Space between the anchor and the card, and the closest the card comes to the viewport's edge. */
  const GAP = 6;
  const MARGIN = 8;

  const join: Attachment<HTMLElement> = (element) =>
    untrack(() => {
      const from = anchor ?? null;
      const takesFocus = initialFocus !== false;
      const layer = stack.open({
        kind: "float",
        element,
        dismiss: () => onclose(),
        anchor: from,
        returnFocus: takesFocus ? from : false,
      });

      const place = (): void => {
        // A trigger that went away (its row re-rendered) leaves nothing to sit beside.
        if (!from?.isConnected) {
          onclose();
          return;
        }
        const viewport = document.documentElement;
        const placed = placeFloat(
          from.getBoundingClientRect(),
          { width: element.offsetWidth, height: element.scrollHeight },
          { width: viewport.clientWidth, height: viewport.clientHeight },
          { side, align, gap: GAP, margin: MARGIN },
        );
        element.style.top = `${String(placed.top)}px`;
        element.style.left = `${String(placed.left)}px`;
        element.style.setProperty("--float-room", `${String(placed.room)}px`);
        element.dataset.side = placed.side;
      };

      // Tab past either end leaves the layer for its trigger; Tab in a menu,
      // whose items take focus by arrow keys, always does.
      const tabOut = (event: KeyboardEvent): void => {
        if (event.key !== "Tab" || event.defaultPrevented || event.isComposing) return;
        const items = focusables(element);
        const active = document.activeElement;
        const atEdge = event.shiftKey
          ? active === element || active === items[0]
          : active === items.at(-1);
        if (items.length > 0 && !atEdge) return;
        event.preventDefault();
        from?.focus();
        onclose();
      };

      place();
      if (typeof initialFocus === "function") initialFocus(element);
      else if (takesFocus) focusInitial(element, initialFocus);

      const resized = typeof ResizeObserver === "function" ? new ResizeObserver(place) : null;
      resized?.observe(element);
      window.addEventListener("resize", place);
      window.addEventListener("scroll", place, { capture: true, passive: true });
      element.addEventListener("keydown", tabOut);
      return () => {
        resized?.disconnect();
        window.removeEventListener("resize", place);
        window.removeEventListener("scroll", place, { capture: true });
        element.removeEventListener("keydown", tabOut);
        layer.release();
      };
    });
</script>

{#if open}
  <!-- The layer moves into the overlay host; these hold its place in the block. -->
  <template></template>
  <div
    {...rest}
    class={["ui-float", className]}
    data-shape={shape}
    data-side={side}
    tabindex="-1"
    style:--float-width={width}
    {@attach portal}
    {@attach join}
  >
    {@render children()}
  </div>
  <template></template>
{/if}

<style>
  .ui-float {
    /* Quiet buttons hover to stone-4 on the stone-3 card. */
    --ui-quiet-hover: var(--color-stone-4);
    --float-from: translateY(-4px);

    position: fixed;
    top: 0;
    left: 0;
    z-index: var(--z-overlay);
    max-width: calc(100vw - 2 * var(--space-8));
    overflow-y: auto;
    overscroll-behavior: contain;
    background: var(--color-stone-3);
    border-radius: var(--corner-lg);
    box-shadow: var(--shadow-float);
    animation: ui-float-in var(--duration-base) var(--ease-out);

    &:focus-visible {
      outline: none;
    }
  }

  /* It leans in from the side it opens toward. */
  .ui-float[data-side="top"] {
    --float-from: translateY(4px);
  }

  .ui-float[data-side="left"] {
    --float-from: translateX(4px);
  }

  .ui-float[data-side="right"] {
    --float-from: translateX(-4px);
  }

  .ui-float:is([data-side="top"], [data-side="bottom"]) {
    max-height: var(--float-room);
  }

  .ui-float[data-shape="menu"] {
    min-width: 200px;
    max-width: min(320px, calc(100vw - 2 * var(--space-8)));
    padding: var(--space-6);
  }

  .ui-float[data-shape="popover"] {
    width: var(--float-width, 300px);
    padding: var(--space-12);
  }

  .ui-float[data-shape="tooltip"] {
    max-width: min(260px, calc(100vw - 2 * var(--space-8)));
    padding: var(--space-4) var(--space-8);
    overflow: hidden;
    border-radius: var(--corner-sm);
    background: var(--color-stone-4);
    color: var(--color-text);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-tight);
    pointer-events: none;
    animation-duration: var(--duration-fast);
  }

  @keyframes ui-float-in {
    from {
      opacity: 0;
      transform: var(--float-from);
    }
  }
</style>
