# Residuum — Visual System

Dark stone surfaces, one blue vein for accent and focus, and compact, quiet density. Grouping comes from surface tone and spacing rather than boxes. Color is spent on state: what is working, what needs the user, what failed.

## Tokens

Every value lives in `src/styles/tokens.css`, the only stylesheet allowed to spell out literal colors, font sizes, z-indexes, durations and easing curves. Stylelint enforces this for every other stylesheet and component. Reference tokens with `var(--…)`; if a value is missing, add a token rather than a literal.

| Family | Prefix | Examples |
|---|---|---|
| Colors | `--color-` | `--color-stone-2`, `--color-text-2`, `--color-vein-tint` |
| Floating shadow | `--shadow-float` | |
| Font families | `--font-` | `--font-ui`, `--font-code`, `--font-mark` |
| Type scale | `--font-size-`, `--font-weight-`, `--line-height-` | `--font-size-message`, `--font-weight-medium` |
| Radii | `--corner-` | `--corner-sm`, `--corner-lg`, `--corner-pill` |
| Spacing | `--space-` + pixels | `--space-8`, `--space-18` |
| Layout | `--layout-` | `--layout-rail-width`, `--layout-bottom-bar-offset` |
| Breakpoints | `--breakpoint-` | `--breakpoint-phone-max` |
| Stacking | `--z-` | `--z-drawer`, `--z-toast` |
| Motion | `--duration-`, `--ease-` | `--duration-fast`, `--ease-out` |
| Focus | `--focus-outline-` | `--focus-outline-width` |

The palette is fixed. Tints are the palette colors at an alpha (`color-mix`), never new hues.

## Surfaces and elevation

| Surface | Used for |
|---|---|
| `stone-0` | The page: main region, chat feed |
| `stone-1` | The rail, the context panel, cards and settings groups |
| `stone-2` | Wells inside those: the composer, the search field, code and step details |
| `stone-3` | Floating layers (menus, popovers, dialogs, the palette) and hover on stone-1 rows |
| `stone-4` | Hover and selected fills, mostly inside floating layers; toasts |
| `input` | Text field fill |

- Borders appear only on controls (`control-border`) and on the hairlines between regions (`line`, `line-soft`). Cards and groups have no outline.
- Only floating layers carry `--shadow-float`. Modal layers sit on `--color-scrim`.
- Tints mark state: `vein-tint` for the selected row, `vein-faint` for informational banners and pills, `moss-tint` for the user's messages, `err-tint` for problem rows and banners.

## Color and contrast

`src/styles/contrast-pairs.ts` declares every allowed foreground/surface pair, and `tokens.test.ts` checks each against WCAG AA: 4.5:1 for text, 3:1 for large text, glyphs and control boundaries. A pair that is not declared is not allowed, even if it happens to pass.

- `text` and `text-2` sit on any stone surface.
- `text-3`, `vein` as text, `vein-bright`, `moss-text` and `err-text` sit on `stone-0` to `stone-3` and the input fill, never on `stone-4`. `text-3` is the dimmest text and the placeholder color.
- White (`on-accent`) sits only on `vein-dim` and `vein-hover`, so primary buttons fill with `vein-dim`. A solid count badge is `stone-0` on `vein-bright`.
- On a tint, text uses only `text`, `text-2` or `vein-bright`; selected rows label in `vein-bright`. Over `stone-4`, a tint takes `text` and `vein-bright` only.
- Icons, status marks and the focus outline may use any text color on any surface.
- Field boundaries use `control-border` at rest, `vein` when focused and `err` when invalid.

## Type

Onest for all UI and message text, JetBrains Mono only for code, file paths and ids, Cinzel only in the wordmark. The fonts are bundled (`src/styles/fonts.css`) and load only weights 400, 500 and 600 (Onest), 400 and 500 (JetBrains Mono) and 500 (Cinzel); any other weight is synthesized, so don't use one.

| Role | Size | Weight | Line height | Used for |
|---|---|---|---|---|
| Title | `title` 20px | semibold | `tight` | Page and settings-section titles |
| Heading | `heading` 17px | semibold | `tight` | Section headings, dialog and state-card titles |
| Message | `message` 15px | regular | `message` | Chat messages and agent prose |
| UI | `ui` 14px | regular or medium | `ui` | Default text, controls, labels |
| Secondary | `sm` 13px | regular to semibold | `ui` | Rows, rail places, section labels on pages, help text |
| Meta | `xs` 12px | regular | `ui` | Timestamps, counts, group labels |
| Code | `code` 0.86em inline, `xs` in blocks | regular or medium | inherited | Code, paths, ids |

The wordmark is Cinzel 500 in capitals with wide letter spacing, next to the `mark` icon in `vein`. Nothing else uses Cinzel or capitals.

## Shape and density

- Radii: `corner-sm` (6px) for controls and rows, `corner-md` (8px) for list items and small cards, `corner-lg` (12px) for groups, panels and floating layers, `corner-pill` for badges and pills.
- Density is compact: 32px rows and buttons at wide widths. At phone width every control grows to `--layout-touch-target` (44px), and text fields use `--font-size-field-phone` (16px) so iOS doesn't zoom into them.
- Space comes from the `--space-` scale.

## Layout

- The rail is `--layout-rail-width`. The context panel opens at `--layout-panel-width` and resizes between `--layout-panel-min-width` and `--layout-panel-max-width`.
- Conversations cap at `--layout-reading-width`; Home caps at `--layout-home-width`, centered.
- On phones the bottom bar is `--layout-bottom-bar-height` plus the safe-area inset. Anything pinned to the bottom sits above `--layout-bottom-bar-offset`.
- Shell breakpoints: phone up to 760px, medium 761–1180px (the panel floats over the main region), wide from 1181px (the panel sits beside it). Shell `@media` rules write these widths out as `max-width`/`min-width`, and lint rejects any other width. Scripts use `src/styles/breakpoints.ts`. A component that needs its own responsive rule uses a container query.
- Stacking, bottom to top: `base`, `sticky`, `panel`, `drawer`, `overlay`, `palette`, `toast`.

## Motion

Motion answers what the user did, and shows what changed.

- `--duration-fast` (150ms) for hover, press and color; `--duration-base` (200ms) for anything that enters, leaves, expands or moves. Both use `--ease-out`.
- Continuous indicators use their own tokens: `spin` for spinners, `pulse` for the working glow, `flow` for the rail's working vein, `blink` for the streaming caret.
- Nothing moves on its own otherwise: no ambient drift, no staggered entrances, no texture.
- Under `prefers-reduced-motion`, a global rule in `src/styles/ui-base.css` makes every animation and transition in the app finish at once.

## Icons

`Icon` from `src/lib/icons` draws every icon on a 24-unit grid with a 1.6-unit stroke and round caps and joins; the set is in `icons.ts`. Icons are decorative (`aria-hidden`), so the button or text beside them carries the label.

- Sizes: 13–14px in dense rows and inline with small text, 15px in the rail and menus, 16px in buttons (the default), 18px in place headers, 21px in the phone bottom bar.
- Icons take `currentColor`; color them through the parent.
- A new icon is drawn on the same grid with the same stroke, from paths, circles and rectangles. Filled shapes are marked `solid`.

## Base styles

`src/styles/ui-base.css` holds the reset and base styles for components: box sizing, zeroed margins and padding, controls that inherit type, headings that take their size from the component, code in JetBrains Mono, vein-bright links, the `vein-bright` focus outline, the placeholder color, and bare dialogs and popovers. It applies inside the element marked `data-ui` (the shell root) and skips anything inside a `data-legacy-view` wrapper, where a hosted legacy view keeps the legacy global styles. Every base rule has zero specificity, so a component's own rule always wins.

The legacy global stylesheets still define generic classes such as `.btn`, `.select`, `.icon-btn` and `.header`. Those match any element with that class, scoped component or not, so components use class names the legacy stylesheets don't define.
