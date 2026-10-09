<script lang="ts">
  import { untrack, type Snippet } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import { router } from "../router.svelte";
  import { focusInitial, portal } from "./overlay/host";
  import { stack, type LayerHandle } from "./overlay/stack";
  import { swipeToDismiss } from "./overlay/swipe";
  import type { ModalFrame } from "./types";

  // The base every modal overlay is drawn on: Dialog, Sheet, Drawer, and
  // surfaces with their own card such as the palette and the Settings modal.
  // It is controlled: it asks to close through `onclose`, and closes when the
  // owner sets `open` false.

  interface Props {
    open: boolean;
    /** `center` hangs below the top edge (dialogs), `top` hangs higher (the palette), `bottom` is a sheet, `left` a drawer, `viewer` is centered in the view with room for a picture. */
    frame?: ModalFrame;
    /** A centered layer fills the screen at phone width. */
    fullscreenOnPhone?: boolean;
    /** Filling the screen on a phone, stop above the bottom bar (`[data-bottom-bar]`), which stays in reach. */
    keepBottomBar?: boolean;
    /** The card's width, for the centered frames. */
    width?: string;
    role?: "dialog" | "alertdialog";
    label?: string;
    labelledby?: string;
    describedby?: string;
    /** What takes focus on open: a selector's match, or false for the card itself. By default `[data-autofocus]`, else the first control. */
    initialFocus?: string | false;
    /**
     * Push an overlay entry, so Back closes the layer. Turn it off for a layer
     * whose URL parameter is its entry, such as the Settings modal.
     */
    historyEntry?: boolean;
    /** The user asked to close it: Esc, the scrim, a swipe, or Back. */
    onclose: () => void;
    class?: string;
    children: Snippet;
  }

  let {
    open,
    frame = "center",
    fullscreenOnPhone = false,
    keepBottomBar = false,
    width = "480px",
    role = "dialog",
    label,
    labelledby,
    describedby,
    initialFocus,
    historyEntry = true,
    onclose,
    class: className,
    children,
  }: Props = $props();

  let layer: LayerHandle | null = null;
  let pressedScrim = false;

  const join: Attachment<HTMLElement> = (element) =>
    untrack(() => {
      let entryLeft = false;
      const entry = historyEntry
        ? router.openOverlay(() => {
            entryLeft = true;
            onclose();
          })
        : null;
      const joined = stack.open({
        kind: "modal",
        element,
        dismiss: () => onclose(),
        spare: keepBottomBar ? document.querySelector("[data-bottom-bar]") : null,
      });
      layer = joined;
      const card = element.querySelector<HTMLElement>(".ui-modal");
      if (card !== null) focusInitial(card, initialFocus);
      return () => {
        layer = null;
        joined.release();
        if (!entryLeft) entry?.close();
      };
    });

  const swipe = $derived.by((): Attachment<HTMLElement> | undefined => {
    if (frame !== "bottom" && frame !== "left") return undefined;
    return swipeToDismiss(frame === "bottom" ? "down" : "left", () => onclose());
  });
</script>

{#if open}
  <!-- The layer moves into the overlay host; these hold its place in the block. -->
  <template></template>
  <div
    class="ui-modal-layer"
    data-frame={frame}
    data-fullscreen={fullscreenOnPhone || undefined}
    data-keep-bar={(fullscreenOnPhone && keepBottomBar) || undefined}
    {@attach portal}
    {@attach join}
  >
    <!-- A pointer target only: Esc closes the layer from the keyboard. -->
    <div
      class="ui-scrim"
      data-overlay-scrim
      aria-hidden="true"
      onpointerdown={(event) => {
        pressedScrim = layer?.topmost === true && !stack.caughtBy(event);
      }}
      onclick={() => {
        if (pressedScrim) onclose();
        pressedScrim = false;
      }}
    ></div>
    <dialog
      open
      aria-modal="true"
      role={role === "alertdialog" ? "alertdialog" : undefined}
      aria-label={label}
      aria-labelledby={labelledby}
      aria-describedby={describedby}
      tabindex="-1"
      class={["ui-modal", className]}
      style:--modal-width={width}
      {@attach swipe}
    >
      {@render children()}
    </dialog>
  </div>
  <template></template>
{/if}

<style>
  .ui-modal-layer {
    position: fixed;
    inset: 0;
    z-index: var(--z-overlay);
    display: flex;
    flex-direction: column;
    align-items: center;
    overscroll-behavior: contain;
  }

  .ui-scrim {
    position: absolute;
    inset: 0;
    background: var(--color-scrim);
    animation: ui-fade var(--duration-base) var(--ease-out);
  }

  .ui-modal {
    /* Quiet buttons hover to stone-4 on the stone-3 card. */
    --ui-quiet-hover: var(--color-stone-4);

    position: relative;
    inset: auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
    max-width: 100%;
    margin: 0;
    overflow: hidden;
    background: var(--color-stone-3);
    box-shadow: var(--shadow-float);
    transition: transform var(--duration-base) var(--ease-out);

    &:focus-visible {
      outline: none;
    }

    /* Set by the swipe while the card follows a finger. */
    &:global([data-swiping]) {
      transition: none;
    }
  }

  /* ── Centered: dialogs ─────────────────────────────────────────────── */

  .ui-modal-layer:is([data-frame="center"], [data-frame="top"]) {
    padding: 14vh max(var(--space-16), var(--safe-right)) max(var(--space-16), var(--safe-bottom))
      max(var(--space-16), var(--safe-left));

    & .ui-modal {
      width: var(--modal-width);
      max-height: 100%;
      border-radius: var(--corner-lg);
      animation: ui-rise var(--duration-base) var(--ease-out);
    }
  }

  /* A search whose results grow downward starts higher and stops short of the bottom. */
  .ui-modal-layer[data-frame="top"] {
    padding-top: 12vh;

    & .ui-modal {
      max-height: min(540px, 72vh);
    }
  }

  /* ── Viewer: a picture, centered in the view with a margin all round ── */

  .ui-modal-layer[data-frame="viewer"] {
    justify-content: center;
    padding: max(var(--space-16), var(--safe-top)) max(var(--space-16), var(--safe-right))
      max(var(--space-16), var(--safe-bottom)) max(var(--space-16), var(--safe-left));

    & .ui-modal {
      width: var(--modal-width);
      max-height: 100%;
      border-radius: var(--corner-lg);
      animation: ui-rise var(--duration-base) var(--ease-out);
    }
  }

  /* ── Bottom: sheets ────────────────────────────────────────────────── */

  .ui-modal-layer[data-frame="bottom"] {
    justify-content: flex-end;

    & .ui-modal {
      width: min(100%, 640px);
      max-height: 84dvh;
      padding: 0 var(--safe-right) var(--safe-bottom) var(--safe-left);
      border-radius: var(--corner-lg) var(--corner-lg) 0 0;
      animation: ui-sheet-in var(--duration-base) var(--ease-out);
    }
  }

  /* ── Left: drawers ─────────────────────────────────────────────────── */

  .ui-modal-layer[data-frame="left"] {
    align-items: flex-start;

    & .ui-modal {
      --ui-quiet-hover: var(--color-stone-3);

      width: min(320px, 86vw);
      height: 100%;
      padding: var(--safe-top) 0 var(--safe-bottom) var(--safe-left);
      overflow-y: auto;
      background: var(--color-stone-1);
      touch-action: pan-y;
      animation: ui-drawer-in var(--duration-base) var(--ease-out);
    }
  }

  @media (max-width: 760px) {
    .ui-modal-layer:is([data-frame="center"], [data-frame="top"])[data-fullscreen] {
      padding: 0;

      & .ui-modal {
        width: 100%;
        height: 100%;
        max-height: 100%;
        padding: var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left);
        border-radius: 0;
        animation-name: ui-sheet-in;
      }
    }

    .ui-modal-layer[data-keep-bar] {
      bottom: var(--layout-bottom-bar-offset);

      & .ui-modal {
        padding-bottom: 0;
      }
    }
  }

  @keyframes ui-fade {
    from {
      opacity: 0;
    }
  }

  @keyframes ui-rise {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }

  @keyframes ui-sheet-in {
    from {
      opacity: 0;
      transform: translateY(24px);
    }
  }

  @keyframes ui-drawer-in {
    from {
      transform: translateX(-100%);
    }
  }
</style>
