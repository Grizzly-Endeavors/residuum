import { describe, expect, it } from "vitest";
import { placeFloat, type PlaceOptions, type Rect } from "./placement";

const VIEWPORT = { width: 1000, height: 800 };
const MENU = { width: 200, height: 300 };
const BELOW: PlaceOptions = { side: "bottom", align: "start", gap: 6, margin: 8 };

/** A 32px button at (x, y). */
function button(x: number, y: number): Rect {
  return { left: x, top: y, width: 32, height: 32 };
}

describe("placeFloat", () => {
  it("opens on the side asked for when it fits there", () => {
    expect(placeFloat(button(100, 100), MENU, VIEWPORT, BELOW)).toEqual({
      side: "bottom",
      top: 138,
      left: 100,
      room: 800 - 132 - 6 - 8,
    });
  });

  it("flips to the opposite side when only that side fits", () => {
    const placed = placeFloat(button(100, 700), MENU, VIEWPORT, BELOW);
    expect(placed.side).toBe("top");
    expect(placed.top).toBe(700 - 6 - 300);
  });

  it("takes the roomier side and caps its size when neither side fits", () => {
    const tall = { width: 200, height: 760 };
    const placed = placeFloat(button(100, 500), tall, VIEWPORT, BELOW);
    expect(placed).toMatchObject({ side: "top", room: 500 - 6 - 8, top: 8 });
    const lower = placeFloat(button(100, 200), tall, VIEWPORT, BELOW);
    expect(lower).toMatchObject({ side: "bottom", room: 800 - 232 - 6 - 8, top: 238 });
  });

  it("lines up with the anchor's start, center or end", () => {
    const anchor = button(400, 100);
    expect(placeFloat(anchor, MENU, VIEWPORT, { ...BELOW, align: "center" }).left).toBe(316);
    expect(placeFloat(anchor, MENU, VIEWPORT, { ...BELOW, align: "end" }).left).toBe(232);
  });

  it("shifts along its side to stay inside the viewport", () => {
    expect(placeFloat(button(960, 100), MENU, VIEWPORT, BELOW).left).toBe(1000 - 8 - 200);
    expect(placeFloat(button(0, 100), MENU, VIEWPORT, { ...BELOW, align: "end" }).left).toBe(8);
    // Wider than the viewport allows: it starts at the margin.
    const wide = placeFloat(button(100, 100), { width: 1200, height: 100 }, VIEWPORT, BELOW);
    expect(wide.left).toBe(8);
  });

  it("opens to the left or right, flipping the same way", () => {
    const tip = { width: 120, height: 24 };
    const right = placeFloat(button(100, 400), tip, VIEWPORT, {
      ...BELOW,
      side: "right",
      align: "center",
    });
    expect(right).toMatchObject({ side: "right", left: 138, top: 404 });
    const flipped = placeFloat(button(900, 400), tip, VIEWPORT, { ...BELOW, side: "right" });
    expect(flipped).toMatchObject({ side: "left", left: 900 - 6 - 120 });
  });
});
