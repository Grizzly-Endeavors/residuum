import { getContext, setContext } from "svelte";

export type PanelLayout = "wide" | "medium" | "phone";

export function panelLayout(widths: { phone: boolean; wide: boolean }): PanelLayout {
  if (widths.phone) return "phone";
  return widths.wide ? "wide" : "medium";
}

export interface PanelFrame {
  readonly layout: PanelLayout;
  readonly titleId: string;
  close: () => void;
}

const FRAME = Symbol("context-panel-frame");

export function providePanelFrame(frame: PanelFrame): void {
  setContext(FRAME, frame);
}

export function panelFrame(): PanelFrame {
  const frame = getContext<PanelFrame | undefined>(FRAME);
  if (frame === undefined) throw new Error("a panel header is rendered outside the context panel");
  return frame;
}
