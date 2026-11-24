export const PANEL_DEFAULT_WIDTH = 440;
export const PANEL_MIN_WIDTH = 360;
export const PANEL_MAX_SHARE = 0.5;

export const PANEL_STEP = 16;
export const PANEL_LARGE_STEP = 64;

const STORAGE_KEY = "residuum-panel-width";

export function panelMaxWidth(viewportWidth: number): number {
  return Math.max(PANEL_MIN_WIDTH, Math.floor(viewportWidth * PANEL_MAX_SHARE));
}

export function clampPanelWidth(width: number, viewportWidth: number): number {
  if (!Number.isFinite(width)) return Math.min(PANEL_DEFAULT_WIDTH, panelMaxWidth(viewportWidth));
  return Math.min(panelMaxWidth(viewportWidth), Math.max(PANEL_MIN_WIDTH, Math.round(width)));
}

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

export function readPanelWidth(): number | null {
  let stored: string | null;
  try {
    stored = localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
  if (stored === null) return null;
  const width = Number(stored);
  return Number.isFinite(width) && width > 0 ? width : null;
}

export function savePanelWidth(width: number): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(Math.round(width)));
  } catch {}
}
