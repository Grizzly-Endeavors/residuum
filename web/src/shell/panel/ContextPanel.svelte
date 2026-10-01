<script lang="ts">
  import type { Snippet } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import { MediaQuery } from "svelte/reactivity";
  import { router } from "../../lib/router.svelte";
  import { ModalLayer } from "../../lib/ui";
  import { PHONE_QUERY, WIDE_QUERY } from "../../styles/breakpoints";
  import { panelLayout, providePanelFrame } from "./panel-frame";
  import {
    clampPanelWidth,
    PANEL_DEFAULT_WIDTH,
    PANEL_MIN_WIDTH,
    panelMaxWidth,
    panelWidthForKey,
    readPanelWidth,
    savePanelWidth,
  } from "./panel-width";

  // The context panel's frame (design §2): a resizable column beside the main
  // region at wide widths, floating over its right edge at medium widths, and
  // a full-screen sheet on phones. The URL's `panel` parameter is its history
  // entry, so closing goes through the router, and Back closes it. What it
  // shows is its children, which start with a `PanelHeader`.

  let { children }: { children: Snippet } = $props();

  const uid = $props.id();
  const titleId = `${uid}-title`;
  const panelId = `${uid}-panel`;

  const phone = new MediaQuery(PHONE_QUERY);
  const wide = new MediaQuery(WIDE_QUERY);
  const layout = $derived(panelLayout({ phone: phone.current, wide: wide.current }));

  let viewportWidth = $state(window.innerWidth);
  /** The width the viewer chose; the panel shows it within what this viewport allows. */
  let chosenWidth = $state(readPanelWidth() ?? PANEL_DEFAULT_WIDTH);
  const width = $derived(clampPanelWidth(chosenWidth, viewportWidth));
  let resizing = $state(false);
  let column = $state<HTMLElement>();

  function close(): void {
    void router.closePanel();
  }

  providePanelFrame({
    get layout() {
      return layout;
    },
    titleId,
    close,
  });

  function closeOnEscape(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
    // A legacy dialog drawn inside the panel takes Esc for itself.
    if (event.target instanceof Element && event.target.closest('[aria-modal="true"]')) return;
    event.preventDefault();
    close();
  }

  /**
   * Beside or over the main region, the panel takes focus when it opens, Esc
   * inside it closes it, and focus goes back to where it was when it closes.
   * On phones the sheet's modal layer does all three.
   */
  const holdFocus: Attachment<HTMLElement> = (element) => {
    const before = document.activeElement;
    const returnTo =
      before instanceof HTMLElement && before !== document.body && !element.contains(before)
        ? before
        : null;
    element.focus();
    element.addEventListener("keydown", closeOnEscape);
    return () => {
      element.removeEventListener("keydown", closeOnEscape);
      const active = document.activeElement;
      const lost = active === null || active === document.body || element.contains(active);
      if (lost && returnTo?.isConnected && returnTo.closest("[inert]") === null) returnTo.focus();
    };
  };

  function startResize(event: PointerEvent): void {
    if (event.button !== 0) return;
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    resizing = true;
  }

  function resize(event: PointerEvent): void {
    if (!resizing || column === undefined) return;
    chosenWidth = clampPanelWidth(
      column.getBoundingClientRect().right - event.clientX,
      viewportWidth,
    );
  }

  function endResize(): void {
    if (!resizing) return;
    resizing = false;
    savePanelWidth(chosenWidth);
  }

  function resizeByKey(event: KeyboardEvent): void {
    const next = panelWidthForKey(event.key, event.shiftKey, width, viewportWidth);
    if (next === null) return;
    event.preventDefault();
    chosenWidth = next;
    savePanelWidth(next);
  }
</script>

<svelte:window bind:innerWidth={viewportWidth} />

{#if layout === "phone"}
  <ModalLayer
    open
    historyEntry={false}
    fullscreenOnPhone
    labelledby={titleId}
    initialFocus={false}
    class="context-panel-sheet"
    onclose={close}
  >
    {@render children()}
  </ModalLayer>
{:else}
  <aside
    id={panelId}
    class="context-panel"
    class:is-resizing={resizing}
    data-layout={layout}
    aria-labelledby={titleId}
    tabindex="-1"
    style:--context-panel-width="{width}px"
    bind:this={column}
    {@attach holdFocus}
  >
    {#if layout === "wide"}
      <!-- A focusable separator is ARIA's window splitter, a widget; Svelte counts every separator as structure. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div
        class="context-panel-grip"
        role="separator"
        tabindex="0"
        aria-label="Resize panel"
        aria-controls={panelId}
        aria-orientation="vertical"
        aria-valuenow={width}
        aria-valuemin={PANEL_MIN_WIDTH}
        aria-valuemax={panelMaxWidth(viewportWidth)}
        aria-valuetext="{width} pixels wide"
        onpointerdown={startResize}
        onpointermove={resize}
        onpointerup={endResize}
        onpointercancel={endResize}
        onkeydown={resizeByKey}
      ></div>
    {/if}
    {@render children()}
  </aside>
{/if}

<style>
  .context-panel {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--color-stone-1);
    border-left: 1px solid var(--color-line-soft);
    animation: context-panel-in var(--duration-base) var(--ease-out);

    &[data-layout="wide"] {
      width: var(--context-panel-width);
    }

    /* Medium widths: the panel floats over the main region's right edge. */
    &[data-layout="medium"] {
      position: fixed;
      top: 0;
      right: 0;
      bottom: 0;
      z-index: var(--z-panel);
      width: var(--layout-panel-width);
      max-width: calc(100vw - var(--space-64));
      box-shadow: var(--shadow-float);
    }

    /* It takes focus when it opens, as a modal card does, and is named by its title. */
    &:focus-visible {
      outline: none;
    }

    /* Text doesn't select while the edge is dragged across it. */
    &.is-resizing {
      user-select: none;
    }
  }

  /* A hosted legacy view fills the panel under its header. */
  .context-panel > :global([data-legacy-view]),
  :global(.context-panel-sheet > [data-legacy-view]) {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  /* The left edge: a hairline that lights up in the vein while held or focused. */
  .context-panel-grip {
    position: absolute;
    top: 0;
    bottom: 0;
    left: calc(var(--space-4) * -1);
    z-index: var(--z-sticky);
    width: var(--space-8);
    cursor: col-resize;
    touch-action: none;

    &::after {
      position: absolute;
      top: 0;
      bottom: 0;
      left: calc(var(--space-4) - 1px);
      width: 2px;
      background: transparent;
      content: "";
      transition: background-color var(--duration-fast) var(--ease-out);
    }

    &:hover::after,
    .is-resizing > &::after {
      background: var(--color-vein);
    }

    &:focus-visible {
      outline: none;

      &::after {
        background: var(--color-vein-bright);
      }
    }
  }

  /* On phones: the full-screen sheet takes the panel's surface. */
  :global(.ui-modal-layer > .ui-modal.context-panel-sheet) {
    --ui-quiet-hover: var(--color-stone-3);

    background: var(--color-stone-1);
    box-shadow: none;
  }

  @keyframes context-panel-in {
    from {
      opacity: 0;
      transform: translateX(var(--space-12));
    }
  }
</style>
