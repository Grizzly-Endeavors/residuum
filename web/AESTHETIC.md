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
| `stone-4` | Hover and selected fills, mostly inside floating layers; toasts and tooltips |
| `input` | Text field fill |

- Borders appear only on controls (`control-border`) and on the hairlines between regions (`line`, `line-soft`). Cards and groups have no outline.
- Only floating layers carry `--shadow-float`, and the context panel while it floats at medium widths. Modal layers sit on `--color-scrim`.
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

- The rail is `--layout-rail-width`. The context panel opens at `--layout-panel-width` and, beside the main region at wide widths, resizes between `--layout-panel-min-width` and `--layout-panel-max-width` from a hairline on its left edge that lights in `vein` while hovered or dragged and in `vein-bright` while focused. At medium widths it floats over the main region at the default width with the floating shadow, and on phones it is a full-screen sheet over the bottom bar. Its header is at least as tall as a place's header, so the two hairlines meet.
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

## Primitives

`src/lib/ui` holds the controls every surface is built from, exported from its `index.ts`: Button and IconButton; TextField, NumberField, SelectField, Toggle, SegmentedControl and SecretField, which wrap `Field` (label, hint, inline error) around `Input` or their own control; Badge, StatusDot, Disclosure, Tabs, EmptyState, Skeleton, Banner and Kbd; the modal overlays Dialog, Sheet and Drawer, drawn on `ModalLayer`, and ConfirmDialog with `ConfirmHost`; the floating layers Menu (with MenuItem and MenuSeparator), Popover and tooltips, all on `FloatingLayer`, with `TooltipHost` and its `tooltip` provider; `ToastRegion` and the `RecentNotifications` dialog; and the Spinner and VisuallyHidden helpers.

- **Buttons.** Primary fills with `vein-dim` and white; secondary with `stone-3`; quiet has no fill until hovered; danger labels in `err-text` and turns to `text` over its `err-tint` hover. `md` is 32px and `sm` 28px tall. A loading button shows a spinner, reports `aria-busy` and ignores presses but stays focusable. An IconButton takes a required `label`, its accessible name, and asks the tooltip provider in context (`provideTooltips` in `tooltip.ts`, given `tooltip` from `TooltipHost`) to show it; with no provider it shows none.
- **Fields.** The label sits above the control (`stack`), beside it with the control at the end (`row`, the toggle's default), or just before it (`inline`). Hint and error are linked to the control with `aria-describedby`, hint first, and an error also sets `aria-invalid`. The focus ring of a text box or select is its own boundary: `vein` at double weight with a `vein-faint` halo, or `err` with an `err-tint` halo when invalid. Every other control takes the base outline.
- **Keyboard.** Toggles flip with Space and Enter. Segmented controls are radio groups and Tabs a tab list: one tab stop on the chosen item, arrow keys move and choose, skipping disabled items and wrapping, and Home and End jump to the ends. A Disclosure is a button with `aria-expanded`; its closed content stays mounted but hidden.
- **State marks.** StatusDot draws each agent state as its own shape: a `moss-text` dot running, a pulsing `vein-bright` dot working, a `text-3` ring stopped, a turning ring starting (`vein-bright`, clockwise) or stopping (`text-2`, backwards), and an `err-text` triangle failed. Badges on a tint label in `text` or `vein-bright` and carry their tone in a dot; a solid count is `stone-0` on `vein-bright`. Warning and error banners share the `err-tint` wash; errors are announced as alerts, the rest politely.
- **Phones.** At phone width every button, tab, segment, disclosure, text box and select grows to `--layout-touch-target`; the toggle keeps its track and grows its hit area instead.
- **Overlays.** Every overlay renders into one host at the end of `<body>` and joins one stack (`overlay/stack.ts`). Esc closes the topmost layer and goes no further. A modal traps Tab, makes the page and the layers under it inert, stops the page scrolling, and returns focus to what opened it; Back closes it through its overlay entry. Layers stack at `--z-overlay` in the order they open, under the toasts.
- **Modal shapes.** A Dialog hangs 14vh below the top edge, 400, 480 or 640px wide, a `stone-3` card with the floating shadow over the scrim, and fills the screen at phone width when asked. The `top` frame hangs a card higher, 12vh down and at most 540px or 72vh tall, for a search whose results grow downward: the command palette. A Sheet rises from the bottom with a grab handle. A Drawer slides in from the left on `stone-1`. Opening, the scrim fades and the card rises or slides over `--duration-base`; closing is immediate.
- **Swipe.** A sheet dragged down, or a drawer dragged left, follows the finger and closes past a third of its size (120px at most) or on a flick; otherwise it springs back. Under reduced motion it doesn't follow the finger, and the same gesture still closes it.
- **Floating layers.** Menus, popovers and tooltips sit beside what opened them on `FloatingLayer`: a `stone-3` card (a tooltip's is `stone-4`, `xs` text) with the floating shadow. A layer opens on the side it asks for, flips to the other side when only that one has room, shifts along to stay 8px inside the viewport, and scrolls when neither side fits. It leans 4px in from its anchor over `--duration-base` and closes at once. Each joins the overlay stack as a float: Esc and a press outside close the topmost first, a press on its own button toggles it, a modal opening closes it, and it holds no history entry.
- **Menus.** A Menu is a menu button and its items. Enter, Space or Arrow Down open it on the first item, Arrow Up on the last, and a pointer on the menu itself. The arrow keys move and wrap, Home and End jump to the ends, and typing jumps to the next item that starts with the letter typed; letters typed together narrow it. Choosing an item closes the menu and puts focus back on its button, except a checkbox item, which stays open. Tab closes it onto its button too. A disabled item can be reached, so its label is heard, but does nothing. The focused item is the highlight, so pointer and keyboard share one: `stone-4` with `text`, `err-tint` for a danger item, and a hint that brightens from `text-3` to `text-2`. A checkbox item shows a `vein-bright` check when on.
- **Popovers.** A small non-modal dialog of controls beside its button, 300px wide by default. It starts on its first control, and Tab past either end closes it onto its button.
- **Tooltips.** `provideTooltips(tooltip)` at the root of a tree gives every IconButton inside a tooltip of its label, and one mounted `TooltipHost` draws the tooltip that is showing. Any other control can attach `tooltipProvider()?.(text)`. A tooltip shows after a pointer rests 500ms, at once when moving straight on from another, and at once when keyboard focus arrives; leaving, a press, blur or Esc hides it, and hiding never moves focus. Touch shows none. When its text says more than the control's name, it describes the control.
- **Toasts.** `ToastRegion` draws the toast store along the bottom edge, at most 460px wide and centered; on phones it rides above the bottom bar, and above the composer too on a place that has one (its `clearance`). Each toast is a `stone-4` card with its kind's icon (`info` in `text-2`, a `moss-text` check, an `err-text` warning), the message, its action as a `vein-bright` label on `vein-tint` (the one accent stone-4 allows), and a dismiss button. Errors stack above the rest and are announced at once (`role="alert"`); the rest are announced politely. The store sets the timings: 4 seconds, 10 with an action, errors until dismissed. The region lives in the overlay host at `--z-toast`, never joins the stack and is never inert, so an Undo stays in reach above a dialog.
- **Recent notifications.** A Dialog listing what was surfaced since the page opened, newest first: each with its kind's icon, the message, how long ago (moving on each minute), and Details in a quiet disclosure. Clear all offers Undo twice, in the dialog's footer, where focus stays, and on the toast it raises; either one restores the list once.
- **On floating cards.** Quiet buttons hover to `stone-4` on a `stone-3` card (`--ui-quiet-hover`), where the secondary fill wouldn't show: a second action there is quiet. A Kbd, `stone-3` elsewhere, takes `stone-4` and `text-2` on a modal card so the key still shows. A confirm dialog puts its action last, as primary or as danger when it removes or loses something, and starts focus on the safer choice.
- **Class names.** Every primitive class starts with `ui-`, which no legacy stylesheet uses.

The gallery at `/dev/gallery` shows every primitive in every state, live. It is served by the dev server (`npm run dev`, `npm run dev:mock`) and built into mock builds (`VITE_MOCK=1`); production builds compile it out, through the `__UI_GALLERY__` flag set in `vite.config.ts`.

## Base styles

`src/styles/ui-base.css` holds the reset and base styles for components: box sizing, zeroed margins and padding, controls that inherit type, headings that take their size from the component, code in JetBrains Mono, vein-bright links, the `vein-bright` focus outline, the placeholder color, and bare dialogs and popovers. It applies inside the element marked `data-ui` (the shell root) and skips anything inside a `data-legacy-view` wrapper, where a hosted legacy view keeps the legacy global styles. Every base rule has zero specificity, so a component's own rule always wins.

The legacy global stylesheets still define generic classes such as `.btn`, `.select`, `.icon-btn` and `.header`. Those match any element with that class, scoped component or not, so components use class names the legacy stylesheets don't define.
