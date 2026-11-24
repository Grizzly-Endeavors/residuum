// @ts-check

import { readFileSync } from "node:fs";

const TOKEN_FILE = "src/styles/tokens.css";

const BASE_FILE = "src/styles/ui-base.css";

const TOKEN_REFERENCE = /^var\(--[\w-]+\)$/;

const CSS_WIDE_KEYWORDS = ["inherit", "initial", "unset", "revert", "revert-layer"];

const LITERAL_COLOR_FUNCTION = /^(rgba?|hsla?|hwb|lab|lch|oklab|oklch|color|device-cmyk)$/i;

const DURATION_LITERAL = /(^|[\s,(*/+])(\d+\.?\d*|\.\d+)m?s\b/;

const EASING_LITERAL =
  /(^|[\s,])(ease|ease-in|ease-out|ease-in-out|linear|step-start|step-end)($|[\s,])|\b(cubic-bezier|steps|linear)\(/;

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
  overrides: [
    { files: ["**/*.svelte"], customSyntax: "postcss-html" },
    { files: [BASE_FILE], rules: { "property-disallowed-list": null } },
  ],
  rules: {
    "color-no-hex": [true, { message: "Use a color token (var(--…)) instead of a hex color." }],
    "color-named": ["never", { message: "Use a color token (var(--…)) instead of a named color." }],
    "function-disallowed-list": [
      [LITERAL_COLOR_FUNCTION],
      { message: "Use a color token (var(--…)) instead of a literal color function." },
    ],

    "declaration-property-value-allowed-list": [
      {
        "font-size": [TOKEN_REFERENCE, ...CSS_WIDE_KEYWORDS],
        font: [TOKEN_REFERENCE, ...CSS_WIDE_KEYWORDS],
        "z-index": [TOKEN_REFERENCE, "auto", ...CSS_WIDE_KEYWORDS],
      },
      {
        message: (property, value) =>
          `Use a design token (var(--…)) for ${property}, not the literal ${value}.`,
      },
    ],

    "declaration-property-value-disallowed-list": [
      {
        "/^(transition|animation)(-(duration|delay|timing-function))?$/": [
          DURATION_LITERAL,
          EASING_LITERAL,
        ],
        "/^transition(-property)?$/": [TRANSITION_ALL],
        "/^--/": [DURATION_LITERAL, EASING_LITERAL],
      },
      {
        message: (property, value) =>
          `${property}: ${value} is not allowed. Use motion tokens (var(--duration-…), var(--ease-…)), and name the properties to transition instead of "all".`,
      },
    ],

    // A line-height set on an element is a fraction of a pixel at most font
    // sizes, which sets off WebKit's scroll anchoring, and it hands the
    // element's children a box for the wrong size. Components set the
    // inherited factor instead.
    "property-disallowed-list": [
      ["line-height"],
      {
        message: `Set the leading factor as --leading: var(--line-height-…), or a number, instead of line-height; ${BASE_FILE} turns it into a whole-pixel line box.`,
      },
    ],

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
