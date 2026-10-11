/**
 * The color pairs the palette allows, part of the token set in tokens.css.
 * A text or glyph color sits on a surface only as declared here, and
 * tokens.test.ts checks every pair against WCAG AA.
 */

/** A color custom property declared in tokens.css. */
export type ColorToken = `--color-${string}`;

/** What the foreground is, which sets the contrast it needs. */
export type ContrastUse = "text" | "large-text" | "non-text";

/** WCAG AA minimums: text, large text, and glyphs and control boundaries. */
export const MIN_CONTRAST: Readonly<Record<ContrastUse, number>> = {
  text: 4.5,
  "large-text": 3,
  "non-text": 3,
};

/** An opaque color, or a translucent tint composited over one. */
export interface Surface {
  readonly fill: ColorToken;
  readonly over?: ColorToken;
}

export interface ContrastPair {
  readonly foreground: ColorToken;
  readonly surface: Surface;
  readonly use: ContrastUse;
}

const STONES = [
  "--color-stone-0",
  "--color-stone-1",
  "--color-stone-2",
  "--color-stone-3",
  "--color-stone-4",
] as const satisfies readonly ColorToken[];

/** The stones that hold content. stone-4 is a row fill, not a container. */
const CONTAINER_STONES = STONES.slice(0, 4);

const TINTS = [
  "--color-vein-tint",
  "--color-vein-faint",
  "--color-moss-tint",
  "--color-err-tint",
] as const satisfies readonly ColorToken[];

const opaque = (fills: readonly ColorToken[]): Surface[] => fills.map((fill) => ({ fill }));

const tinted = (overs: readonly ColorToken[]): Surface[] =>
  TINTS.flatMap((fill) => overs.map((over) => ({ fill, over })));

const pairs = (
  foregrounds: readonly ColorToken[],
  surfaces: readonly Surface[],
  use: ContrastUse,
): ContrastPair[] =>
  foregrounds.flatMap((foreground) => surfaces.map((surface) => ({ foreground, surface, use })));

const ALL_SURFACES = [
  ...opaque([...STONES, "--color-input"]),
  ...tinted(STONES),
] satisfies Surface[];

export const CONTRAST_PAIRS: readonly ContrastPair[] = [
  // text and text-2 read on every stone and the input fill.
  ...pairs(["--color-text", "--color-text-2"], opaque([...STONES, "--color-input"]), "text"),
  // The dimmest and colored tones read on stone-0 to stone-3 and the input
  // fill, never on stone-4. text-3 is also the placeholder color.
  ...pairs(
    [
      "--color-text-3",
      "--color-vein",
      "--color-vein-bright",
      "--color-moss-text",
      "--color-err-text",
    ],
    opaque([...CONTAINER_STONES, "--color-input"]),
    "text",
  ),
  // White labels sit only on the primary button fills.
  ...pairs(["--color-on-accent"], opaque(["--color-vein-dim", "--color-vein-hover"]), "text"),
  // A solid count badge.
  ...pairs(["--color-stone-0"], opaque(["--color-vein-bright"]), "text"),
  // Tint surfaces take text, text-2 and vein-bright. Selected rows label in
  // vein-bright, not vein.
  ...pairs(
    ["--color-text", "--color-text-2", "--color-vein-bright"],
    tinted(CONTAINER_STONES),
    "text",
  ),
  // Over stone-4, text-2 on vein-tint and moss-tint falls below 4.5:1, so
  // tints over stone-4 take text and vein-bright only.
  ...pairs(["--color-text", "--color-vein-bright"], tinted(["--color-stone-4"]), "text"),
  // Icons, status marks and the vein-bright focus outline may use any text
  // tone on any surface.
  ...pairs(
    [
      "--color-text",
      "--color-text-2",
      "--color-text-3",
      "--color-vein",
      "--color-vein-bright",
      "--color-moss-text",
      "--color-err-text",
    ],
    ALL_SURFACES,
    "non-text",
  ),
  // Control boundaries: at rest, focused and invalid, against the surfaces
  // controls sit on and the input fill they enclose.
  ...pairs(
    ["--color-control-border", "--color-vein", "--color-err"],
    opaque([...CONTAINER_STONES, "--color-input"]),
    "non-text",
  ),
];
