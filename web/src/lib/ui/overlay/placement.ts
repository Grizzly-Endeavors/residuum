/**
 * Where a floating layer (menu, popover, tooltip) sits against what it is
 * anchored to. It takes the side it asks for when it fits there, flips to the
 * opposite side when only that one fits, and otherwise takes the roomier side
 * and scrolls. Along the side it shifts to stay inside the viewport.
 */

import type { FloatAlign, FloatSide } from "../types";

export interface Rect {
  readonly top: number;
  readonly left: number;
  readonly width: number;
  readonly height: number;
}

export interface Size {
  readonly width: number;
  readonly height: number;
}

export interface FloatPlacement {
  readonly side: FloatSide;
  readonly top: number;
  readonly left: number;
  /** The room on the chosen side, along the axis it opens on: cap the layer's size to it. */
  readonly room: number;
}

export interface PlaceOptions {
  readonly side: FloatSide;
  readonly align: FloatAlign;
  /** Space between the anchor and the layer. */
  readonly gap: number;
  /** The closest the layer comes to the viewport's edge. */
  readonly margin: number;
}

const OPPOSITE: Readonly<Record<FloatSide, FloatSide>> = {
  top: "bottom",
  bottom: "top",
  left: "right",
  right: "left",
};

/** Places a layer of `size` beside `anchor`, inside a viewport of `viewport`. */
export function placeFloat(
  anchor: Rect,
  size: Size,
  viewport: Size,
  { side, align, gap, margin }: PlaceOptions,
): FloatPlacement {
  const vertical = side === "top" || side === "bottom";
  // Work along two axes: `main` away from the anchor, `cross` along its side.
  const start = vertical ? anchor.top : anchor.left;
  const end = start + (vertical ? anchor.height : anchor.width);
  const extent = vertical ? size.height : size.width;
  const limit = vertical ? viewport.height : viewport.width;
  const roomOn = (on: FloatSide): number =>
    on === "top" || on === "left" ? start - gap - margin : limit - end - gap - margin;

  const flipped = OPPOSITE[side];
  let chosen = side;
  if (roomOn(side) < extent) {
    if (roomOn(flipped) >= extent || roomOn(flipped) > roomOn(side)) chosen = flipped;
  }
  const room = Math.max(0, roomOn(chosen));
  const main =
    chosen === "top" || chosen === "left" ? start - gap - Math.min(extent, room) : end + gap;

  const crossStart = vertical ? anchor.left : anchor.top;
  const crossLength = vertical ? anchor.width : anchor.height;
  const crossExtent = vertical ? size.width : size.height;
  const crossLimit = vertical ? viewport.width : viewport.height;
  let cross = crossStart;
  if (align === "center") cross = crossStart + (crossLength - crossExtent) / 2;
  else if (align === "end") cross = crossStart + crossLength - crossExtent;
  cross = Math.max(margin, Math.min(cross, crossLimit - margin - crossExtent));

  return vertical
    ? { side: chosen, top: main, left: cross, room }
    : { side: chosen, top: cross, left: main, room };
}
