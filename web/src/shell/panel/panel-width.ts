// The context panel's width at wide widths: it opens at the
// default width, the viewer resizes it between the minimum and half the
// viewport, and the width they chose is remembered in this browser.

/** Mirrors `--layout-panel-width` in tokens.css. */
export const PANEL_DEFAULT_WIDTH = 440;
/** Mirrors `--layout-panel-min-width` in tokens.css. */
export const PANEL_MIN_WIDTH = 360;
/** Mirrors `--layout-panel-max-width` (`50vw`): the widest the panel goes, as a share of the viewport. */
export const PANEL_MAX_SHARE = 0.5;

/** How far an arrow key on the resize handle moves the panel's edge. */
export const PANEL_STEP = 16;
/** How far an arrow key moves it with Shift held. */
export const PANEL_LARGE_STEP = 64;

const STORAGE_KEY = "residuum-panel-width";

/** The widest the panel may be in a viewport this wide. */
export function panelMaxWidth(viewportWidth: number): number {
  return Math.max(PANEL_MIN_WIDTH, Math.floor(viewportWidth * PANEL_MAX_SHARE));
}

/** `width` held between the minimum and the most this viewport allows, in whole pixels. */
export function clampPanelWidth(width: number, viewportWidth: number): number {
  if (!Number.isFinite(width)) return Math.min(PANEL_DEFAULT_WIDTH, panelMaxWidth(viewportWidth));
  return Math.min(panelMaxWidth(viewportWidth), Math.max(PANEL_MIN_WIDTH, Math.round(width)));
}

/**
 * The width a key pressed on the resize handle moves the panel to, or null for
 * a key the handle doesn't use. The handle is the panel's left edge, so Left
 * widens it; Home and End go to the narrowest and the widest.
 */
export function panelWidthForKey(
  key: string,
  shift: boolean,
  width: number,
  viewportWidth: number,
): number | null {
  const step = shift ? PANEL_LARGE_STEP : PANEL_STEP;
  switch (key) {
    case "ArrowLeft":
      return clampPanelWidth(width + step, viewportWidth);
    case "ArrowRight":
      return clampPanelWidth(width - step, viewportWidth);
    case "Home":
      return PANEL_MIN_WIDTH;
    case "End":
      return panelMaxWidth(viewportWidth);
    default:
      return null;
  }
}

/** The width this viewer last chose, or null when there is none to read. */
export function readPanelWidth(): number | null {
  let stored: string | null;
  try {
    stored = localStorage.getItem(STORAGE_KEY);
  } catch {
    // Storage is blocked (a private window, site data off): the panel opens at the default.
    return null;
  }
  if (stored === null) return null;
  const width = Number(stored);
  return Number.isFinite(width) && width > 0 ? width : null;
}

/** Remember the width this viewer chose. */
export function savePanelWidth(width: number): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(Math.round(width)));
  } catch {
    // Storage is blocked or full: the width holds for this page and isn't remembered.
  }
}
