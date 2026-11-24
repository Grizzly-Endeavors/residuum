export type ColorToken = `--color-${string}`;

export type ContrastUse = "text" | "large-text" | "non-text";

export const MIN_CONTRAST: Readonly<Record<ContrastUse, number>> = {
  text: 4.5,
  "large-text": 3,
  "non-text": 3,
};

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
  ...pairs(["--color-text", "--color-text-2"], opaque([...STONES, "--color-input"]), "text"),
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
  ...pairs(["--color-on-accent"], opaque(["--color-vein-dim", "--color-vein-hover"]), "text"),
  ...pairs(["--color-stone-0"], opaque(["--color-vein-bright"]), "text"),
  ...pairs(
    ["--color-text", "--color-text-2", "--color-vein-bright"],
    tinted(CONTAINER_STONES),
    "text",
  ),
  ...pairs(["--color-text", "--color-vein-bright"], tinted(["--color-stone-4"]), "text"),
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
  ...pairs(
    ["--color-control-border", "--color-vein", "--color-err"],
    opaque([...CONTAINER_STONES, "--color-input"]),
    "non-text",
  ),
];
