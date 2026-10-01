// @ts-check

import { readFileSync } from "node:fs";

// Style lint for the web UI's global stylesheets and component <style> blocks.
//
// Outside the token file, styles reference design tokens instead of spelling
// out literal colors, font sizes, z-indexes, durations or easing curves. The
// token file is the one place those literals are declared, so a value changes
// in one place. Viewport media queries use only the shell breakpoints; a
// component that needs its own responsive rule uses a container query.

/** The one file that may declare literal values: the design token set. */
const TOKEN_FILE = "src/styles/tokens.css";

/** A single token reference such as `var(--font-size-ui)`, with no literal fallback. */
const TOKEN_REFERENCE = /^var\(--[\w-]+\)$/;

/** CSS-wide keywords, which never carry a literal value. */
const CSS_WIDE_KEYWORDS = ["inherit", "initial", "unset", "revert", "revert-layer"];

/** Color functions that take literal channel values. */
const LITERAL_COLOR_FUNCTION = /^(rgba?|hsla?|hwb|lab|lch|oklab|oklch|color|device-cmyk)$/i;

/** A duration such as `0.3s` or `200ms`. Token names like `--dur-200ms` are not matched. */
const DURATION_LITERAL = /(^|[\s,(*/+])(\d+\.?\d*|\.\d+)m?s\b/;

/** An easing keyword or function. */
const EASING_LITERAL =
  /(^|[\s,])(ease|ease-in|ease-out|ease-in-out|linear|step-start|step-end)($|[\s,])|\b(cubic-bezier|steps|linear)\(/;

/** `all` as a transitioned property animates everything, including layout. */
const TRANSITION_ALL = /(^|[\s,])all($|[\s,])/;

/**
 * Reads a `--breakpoint-*` width in pixels from the token file, so lint and
 * tokens can't disagree.
 * @param {string} name
 * @returns {number}
 */
function readBreakpoint(name) {
  const tokens = readFileSync(new URL(`./${TOKEN_FILE}`, import.meta.url), "utf8");
  const width = new RegExp(`--breakpoint-${name}:\\s*(\\d+)px;`).exec(tokens)?.[1];
  if (width === undefined) {
    throw new Error(`stylelint config: --breakpoint-${name} is missing from ${TOKEN_FILE}`);
  }
  return Number(width);
}

const PHONE_MAX = readBreakpoint("phone-max");
const WIDE_MIN = readBreakpoint("wide-min");

/** @type {import("stylelint").Config} */
export default {
  ignoreFiles: [TOKEN_FILE],
  overrides: [{ files: ["**/*.svelte"], customSyntax: "postcss-html" }],
  rules: {
    // Literal colors: hex, named, and the functional notations.
    "color-no-hex": [true, { message: "Use a color token (var(--…)) instead of a hex color." }],
    "color-named": ["never", { message: "Use a color token (var(--…)) instead of a named color." }],
    "function-disallowed-list": [
      [LITERAL_COLOR_FUNCTION],
      { message: "Use a color token (var(--…)) instead of a literal color function." },
    ],

    "declaration-property-value-allowed-list": [
      {
        // Raw font sizes.
        "font-size": [TOKEN_REFERENCE, ...CSS_WIDE_KEYWORDS],
        // The `font` shorthand carries a size, so it may only inherit or take a token.
        font: [TOKEN_REFERENCE, ...CSS_WIDE_KEYWORDS],
        // Raw z-index values.
        "z-index": [TOKEN_REFERENCE, "auto", ...CSS_WIDE_KEYWORDS],
      },
      {
        message: (property, value) =>
          `Use a design token (var(--…)) for ${property}, not the literal ${value}.`,
      },
    ],

    "declaration-property-value-disallowed-list": [
      {
        // Literal durations, delays and easing curves.
        "/^(transition|animation)(-(duration|delay|timing-function))?$/": [
          DURATION_LITERAL,
          EASING_LITERAL,
        ],
        // `transition: all` also animates layout properties.
        "/^transition(-property)?$/": [TRANSITION_ALL],
        // The color rules already cover custom properties. Motion literals
        // could otherwise be parked in a local one and referenced from there.
        "/^--/": [DURATION_LITERAL, EASING_LITERAL],
      },
      {
        message: (property, value) =>
          `${property}: ${value} is not allowed. Use motion tokens (var(--duration-…), var(--ease-…)), and name the properties to transition instead of "all".`,
      },
    ],

    // Viewport widths: min-/max- notation, so the allowed list below sees every width.
    "media-feature-range-notation": "prefix",
    "media-feature-name-value-allowed-list": [
      {
        "max-width": [`${PHONE_MAX}px`, `${WIDE_MIN - 1}px`],
        "min-width": [`${PHONE_MAX + 1}px`, `${WIDE_MIN}px`],
      },
      {
        message: (feature, value) =>
          `${feature}: ${value} is not a shell breakpoint. Use a container query for a component's own responsive rules.`,
      },
    ],
  },
};
