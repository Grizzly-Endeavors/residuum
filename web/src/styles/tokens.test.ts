import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { PHONE_MAX_WIDTH, WIDE_MIN_WIDTH } from "./breakpoints";
import {
  CONTRAST_PAIRS,
  MIN_CONTRAST,
  type ColorToken,
  type ContrastPair,
  type Surface,
} from "./contrast-pairs";

interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Custom properties declared in tokens.css, by name. */
function readTokens(): Map<string, string> {
  const css = readFileSync(new URL("./tokens.css", import.meta.url), "utf8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const tokens = new Map<string, string>();
  for (const match of css.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
    const [, name, value] = match;
    if (name !== undefined && value !== undefined) tokens.set(name, value.replace(/\s+/g, " "));
  }
  return tokens;
}

const TOKENS = readTokens();

function tokenValue(name: string): string {
  const value = TOKENS.get(name);
  if (value === undefined) throw new Error(`${name} is not declared in tokens.css`);
  return value;
}

/** Resolves the color syntaxes tokens.css uses; anything else fails the test loudly. */
function parseColor(value: string): Rgba {
  const reference = /^var\((--[\w-]+)\)$/.exec(value);
  if (reference?.[1] !== undefined) return parseColor(tokenValue(reference[1]));

  const hex = /^#([0-9a-f]{6})$/i.exec(value)?.[1];
  if (hex !== undefined) {
    const channel = (i: number): number => parseInt(hex.slice(i, i + 2), 16);
    return { r: channel(0), g: channel(2), b: channel(4), a: 1 };
  }

  const mix = /^color-mix\(in srgb, (.+) ([\d.]+)%, transparent\)$/.exec(value);
  if (mix?.[1] !== undefined && mix[2] !== undefined) {
    return { ...parseColor(mix[1]), a: Number(mix[2]) / 100 };
  }

  const rgb = /^rgb\(([\d.]+) ([\d.]+) ([\d.]+) \/ ([\d.]+)%\)$/.exec(value);
  if (rgb !== null) {
    const [, r, g, b, a] = rgb.map(Number);
    if (r !== undefined && g !== undefined && b !== undefined && a !== undefined) {
      return { r, g, b, a: a / 100 };
    }
  }

  throw new Error(`unsupported color syntax in tokens.css: ${value}`);
}

function opaqueColor(token: ColorToken): Rgba {
  const color = parseColor(tokenValue(token));
  if (color.a !== 1) throw new Error(`${token} is translucent; declare it as a tint over a stone`);
  return color;
}

/** The color a surface renders as: a tint is composited over its stone. */
function surfaceColor(surface: Surface): Rgba {
  if (surface.over === undefined) return opaqueColor(surface.fill);
  const tint = parseColor(tokenValue(surface.fill));
  if (tint.a === 1) throw new Error(`${surface.fill} is opaque; declare it without "over"`);
  const base = opaqueColor(surface.over);
  const blend = (top: number, bottom: number): number => tint.a * top + (1 - tint.a) * bottom;
  return { r: blend(tint.r, base.r), g: blend(tint.g, base.g), b: blend(tint.b, base.b), a: 1 };
}

function relativeLuminance({ r, g, b }: Rgba): number {
  const linear = (channel: number): number => {
    const c = channel / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}

function contrastRatio(a: Rgba, b: Rgba): number {
  const [light, dark] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
  return ((light ?? 0) + 0.05) / ((dark ?? 0) + 0.05);
}

const rgb = (r: number, g: number, b: number): Rgba => ({ r, g, b, a: 1 });

const describeSurface = ({ fill, over }: Surface): string =>
  over === undefined ? fill : `${fill} over ${over}`;

const describePair = ({ foreground, surface, use }: ContrastPair): string =>
  `${foreground} on ${describeSurface(surface)} (${use}, ${MIN_CONTRAST[use]}:1)`;

describe("contrast ratio", () => {
  it("matches WCAG reference values", () => {
    expect(contrastRatio(rgb(255, 255, 255), rgb(0, 0, 0))).toBeCloseTo(21, 5);
    expect(contrastRatio(rgb(0x76, 0x76, 0x76), rgb(255, 255, 255))).toBeCloseTo(4.54, 2);
    expect(contrastRatio(rgb(0x77, 0x77, 0x77), rgb(255, 255, 255))).toBeLessThan(4.5);
  });

  it("is symmetric", () => {
    const text = rgb(0xe8, 0xe8, 0xea);
    const stone = rgb(0x0e, 0x0e, 0x10);
    expect(contrastRatio(text, stone)).toBe(contrastRatio(stone, text));
  });
});

describe("declared contrast pairs", () => {
  it.each(CONTRAST_PAIRS.map((pair) => [describePair(pair), pair] as const))(
    "%s meets WCAG AA",
    (_description, pair) => {
      const ratio = contrastRatio(opaqueColor(pair.foreground), surfaceColor(pair.surface));
      expect(ratio).toBeGreaterThanOrEqual(MIN_CONTRAST[pair.use]);
    },
  );

  const textPairs = CONTRAST_PAIRS.filter((pair) => pair.use !== "non-text");

  it("put white text only on the primary button fills", () => {
    const fills = textPairs
      .filter((pair) => pair.foreground === "--color-on-accent")
      .map((pair) => pair.surface.fill);
    expect(new Set(fills)).toEqual(new Set(["--color-vein-dim", "--color-vein-hover"]));
  });

  it("put only text and text-2 on stone-4", () => {
    const foregrounds = textPairs
      .filter((pair) => pair.surface.fill === "--color-stone-4")
      .map((pair) => pair.foreground);
    expect(new Set(foregrounds)).toEqual(new Set(["--color-text", "--color-text-2"]));
  });

  it("put only text, text-2 and vein-bright on tint surfaces", () => {
    const allowed = new Set(["--color-text", "--color-text-2", "--color-vein-bright"]);
    const onTints = textPairs.filter((pair) => pair.surface.over !== undefined);
    expect(onTints.length).toBeGreaterThan(0);
    for (const pair of onTints) expect(allowed).toContain(pair.foreground);
  });
});

describe("breakpoints", () => {
  it("match the token file", () => {
    expect(tokenValue("--breakpoint-phone-max")).toBe(`${PHONE_MAX_WIDTH}px`);
    expect(tokenValue("--breakpoint-wide-min")).toBe(`${WIDE_MIN_WIDTH}px`);
  });
});
