// @ts-check

import js from "@eslint/js";
import tseslint from "typescript-eslint";
import svelte from "eslint-plugin-svelte";
import svelteParser from "svelte-eslint-parser";
import prettier from "eslint-config-prettier";
import globals from "globals";

export default tseslint.config(
  // ── Global ignores ──────────────────────────────────────────────────
  {
    ignores: ["dist/**", "node_modules/**", "*.config.js", "*.config.ts"],
  },

  // ── Base presets ────────────────────────────────────────────────────
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...svelte.configs["flat/recommended"],
  prettier,

  // ── TypeScript files ────────────────────────────────────────────────
  {
    files: ["src/**/*.ts"],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      // ── Deny: catches real bugs, prevents shortcuts ───────────────
      "no-console": "error",
      "no-debugger": "error",
      "no-alert": "error",
      "no-warning-comments": [
        "error",
        { terms: ["todo", "fixme", "hack", "xxx"], location: "start" },
      ],
      "prefer-const": "error",
      "no-var": "error",
      eqeqeq: ["error", "always", { null: "ignore" }],
      "no-implicit-coercion": "error",
      "no-param-reassign": "error",
      "no-nested-ternary": "error",

      // TypeScript deny rules
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unsafe-assignment": "error",
      "@typescript-eslint/no-unsafe-call": "error",
      "@typescript-eslint/no-unsafe-member-access": "error",
      "@typescript-eslint/no-unsafe-return": "error",
      "@typescript-eslint/no-unsafe-argument": "error",
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": "error",
      "@typescript-eslint/only-throw-error": "error",
      "@typescript-eslint/no-non-null-assertion": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-shadow": "error",
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports" },
      ],
      "@typescript-eslint/restrict-template-expressions": [
        "error",
        { allowNumber: true },
      ],

      // ── Pedantic: stricter discipline ─────────────────────────────
      "@typescript-eslint/explicit-function-return-type": [
        "error",
        { allowExpressions: true },
      ],
      "@typescript-eslint/strict-boolean-expressions": [
        "error",
        {
          allowNullableBoolean: true,
          allowNullableString: true,
          allowNullableNumber: false,
          allowAny: false,
        },
      ],
      "@typescript-eslint/switch-exhaustiveness-check": "error",
      "@typescript-eslint/prefer-nullish-coalescing": [
        "error",
        { ignorePrimitives: { string: true } },
      ],
      "@typescript-eslint/prefer-optional-chain": "error",
      "@typescript-eslint/no-unnecessary-condition": "error",
      "@typescript-eslint/no-dynamic-delete": "off",
      "@typescript-eslint/naming-convention": [
        "error",
        {
          selector: "typeLike",
          format: ["PascalCase"],
        },
      ],

      // Allow empty catch blocks (used for graceful fallbacks)
      "no-empty": ["error", { allowEmptyCatch: true }],
    },
  },

  // ── Svelte files ────────────────────────────────────────────────────
  {
    files: ["src/**/*.svelte", "src/**/*.svelte.ts"],
    languageOptions: {
      globals: globals.browser,
      parser: svelteParser,
      parserOptions: {
        parser: tseslint.parser,
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: [".svelte"],
      },
    },
    rules: {
      // ── Deny rules (same as TS) ──────────────────────────────────
      "no-console": "error",
      "no-debugger": "error",
      "no-alert": "error",
      "no-warning-comments": [
        "error",
        { terms: ["todo", "fixme", "hack", "xxx"], location: "start" },
      ],
      "no-var": "error",
      eqeqeq: ["error", "always", { null: "ignore" }],
      "no-implicit-coercion": "error",
      "no-nested-ternary": "error",

      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unsafe-assignment": "error",
      "@typescript-eslint/no-unsafe-call": "error",
      "@typescript-eslint/no-unsafe-member-access": "error",
      "@typescript-eslint/no-unsafe-return": "error",
      "@typescript-eslint/no-unsafe-argument": "error",
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": "error",
      "@typescript-eslint/only-throw-error": "error",
      "@typescript-eslint/no-non-null-assertion": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-shadow": "error",
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports" },
      ],
      "@typescript-eslint/restrict-template-expressions": [
        "error",
        { allowNumber: true },
      ],

      // ── Pedantic (subset — skip rules that fight runes) ──────────
      "@typescript-eslint/switch-exhaustiveness-check": "error",
      "@typescript-eslint/prefer-nullish-coalescing": [
        "error",
        { ignorePrimitives: { string: true } },
      ],
      "@typescript-eslint/prefer-optional-chain": "error",
      "@typescript-eslint/no-dynamic-delete": "off",
      "@typescript-eslint/naming-convention": [
        "error",
        {
          selector: "typeLike",
          format: ["PascalCase"],
        },
      ],

      // ── Svelte-specific overrides (rules that fight runes) ───────
      "prefer-const": "off",
      "no-undef-init": "off",
      "no-unused-expressions": "off",
      "@typescript-eslint/no-unused-expressions": "off",
      "no-param-reassign": "off",
      "@typescript-eslint/explicit-function-return-type": "off",
      "@typescript-eslint/strict-boolean-expressions": "off",
      "@typescript-eslint/no-unnecessary-condition": "off",
      "@typescript-eslint/no-unnecessary-type-arguments": "off",
      "@typescript-eslint/no-confusing-void-expression": "off",
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-unsafe-return": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-useless-default-assignment": "off",

      // Allow empty catch blocks (used for graceful fallbacks)
      "no-empty": ["error", { allowEmptyCatch: true }],

      // ── Svelte plugin rules ──────────────────────────────────────
      "svelte/no-at-html-tags": "error",
      "svelte/require-each-key": "error",
      "svelte/valid-compile": [
        "error",
        { ignoreWarnings: true },
      ],
      "svelte/no-dom-manipulating": "warn",
      "svelte/no-reactive-reassign": "warn",
    },
  },
);
