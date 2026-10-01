// What the context panel's frame gives the content inside it: how the panel
// is laid out at this width, the id its title takes (which names the panel),
// and how to close it. A panel's content reads it through `PanelHeader`.

import { getContext, setContext } from "svelte";

/**
 * Wide: a column beside the main region, resizable. Medium: floating over the
 * main region's right edge. Phone: a full-screen sheet.
 */
export type PanelLayout = "wide" | "medium" | "phone";

export function panelLayout(widths: { phone: boolean; wide: boolean }): PanelLayout {
  if (widths.phone) return "phone";
  return widths.wide ? "wide" : "medium";
}

export interface PanelFrame {
  readonly layout: PanelLayout;
  /** The id of the panel's title. The panel is labelled by it. */
  readonly titleId: string;
  /** Close the panel, the way closing through the UI does. */
  close: () => void;
}

const FRAME = Symbol("context-panel-frame");

export function providePanelFrame(frame: PanelFrame): void {
  setContext(FRAME, frame);
}

/** The frame of the panel this component is inside. */
export function panelFrame(): PanelFrame {
  const frame = getContext<PanelFrame | undefined>(FRAME);
  if (frame === undefined) throw new Error("a panel header is rendered outside the context panel");
  return frame;
}
