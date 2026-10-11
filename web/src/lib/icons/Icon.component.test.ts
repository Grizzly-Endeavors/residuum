import { describe, expect, it } from "vitest";
import { render } from "../../test/component";
import Icon from "./Icon.svelte";
import { ICONS, type IconName } from "./icons";

const NAMES = Object.keys(ICONS) as IconName[];

function renderIcon(name: IconName, size?: number): SVGSVGElement {
  const { container } = render(Icon, size === undefined ? { name } : { name, size });
  const svg = container.querySelector("svg");
  if (svg === null) throw new Error(`${name} rendered no svg`);
  return svg;
}

describe("Icon", () => {
  it.each(NAMES)("draws %s on the shared grid and stroke", (name) => {
    const svg = renderIcon(name);
    expect(svg.getAttribute("viewBox")).toBe("0 0 24 24");
    expect(svg.getAttribute("stroke-width")).toBe("1.6");
    expect(svg.getAttribute("stroke")).toBe("currentColor");
    expect(svg.getAttribute("aria-hidden")).toBe("true");
    expect(svg.querySelectorAll("path, circle, rect")).toHaveLength(ICONS[name].length);
  });

  it("renders at 16px unless given a size", () => {
    expect(renderIcon("close").getAttribute("width")).toBe("16");
    const large = renderIcon("close", 21);
    expect(large.getAttribute("width")).toBe("21");
    expect(large.getAttribute("height")).toBe("21");
  });

  it("fills solid shapes instead of stroking them", () => {
    const stop = renderIcon("stop").querySelector("rect");
    expect(stop?.getAttribute("fill")).toBe("currentColor");
    expect(stop?.getAttribute("stroke")).toBe("none");

    const close = renderIcon("close").querySelector("path");
    expect(close?.getAttribute("fill")).toBeNull();
    expect(close?.getAttribute("stroke")).toBeNull();
  });

  it("keeps circles and rectangles inside the grid", () => {
    for (const name of NAMES) {
      for (const shape of ICONS[name]) {
        if ("circle" in shape) {
          const [cx, cy, r] = shape.circle;
          expect(
            [cx - r, cy - r].every((edge) => edge >= 0),
            name,
          ).toBe(true);
          expect(
            [cx + r, cy + r].every((edge) => edge <= 24),
            name,
          ).toBe(true);
        } else if ("rect" in shape) {
          const [x, y, width, height] = shape.rect;
          expect(x >= 0 && y >= 0 && x + width <= 24 && y + height <= 24, name).toBe(true);
        }
      }
    }
  });
});
