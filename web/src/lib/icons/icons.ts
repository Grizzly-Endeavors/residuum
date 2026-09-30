/**
 * The icon set, drawn on a 24-unit grid. Icon.svelte strokes every shape at
 * 1.6 units with round caps and joins; a `solid` shape is filled instead.
 */

export type IconShape = (
  | { readonly path: string }
  | { readonly circle: readonly [cx: number, cy: number, r: number] }
  | {
      readonly rect: readonly [x: number, y: number, width: number, height: number, radius: number];
    }
) & { readonly solid?: true };

const DRAWINGS = {
  // ── Places and navigation ─────────────────────────────────────────
  home: [
    { path: "M4 11 12 4.5 20 11" },
    { path: "M6.5 9.5v10h11v-10" },
    { path: "M10 19.5v-5h4v5" },
  ],
  inbox: [{ path: "M4 13.5 6.5 5h11l2.5 8.5V19H4z" }, { path: "M4 13.5h4.5l1.2 2h4.6l1.2-2H20" }],
  chat: [{ path: "M4 5.5h16v11H10l-4.5 3.5v-3.5H4z" }],
  activity: [{ path: "M3 12h4l2.5-6 5 12 2.5-6h4" }],
  clock: [{ circle: [12, 12, 8.5] }, { path: "M12 7.5V12l3 2" }],
  folder: [{ path: "M3.5 6.5h6l2 2h9v10h-17z" }],
  file: [{ path: "M6.5 3.5h7.5l4 4v13h-11.5z" }, { path: "M14 3.5v4h4" }],
  grid: [
    { rect: [4, 4, 7, 7, 1.5] },
    { rect: [13, 4, 7, 7, 1.5] },
    { rect: [4, 13, 7, 7, 1.5] },
    { rect: [13, 13, 7, 7, 1.5] },
  ],
  page: [{ rect: [4, 5, 16, 14, 2] }, { path: "M4 9h16" }],
  users: [
    { circle: [9, 9, 3] },
    { path: "M3.5 19c.5-3 2.8-5 5.5-5s5 2 5.5 5" },
    { circle: [17, 9.5, 2.3] },
    { path: "M16.5 14.2c2.3.2 3.8 1.9 4.2 4.3" },
  ],
  search: [{ circle: [11, 11, 6] }, { path: "m20 20-4.5-4.5" }],
  settings: [
    {
      path: "M12.2 2.5h-.4a1.9 1.9 0 0 0-1.9 1.9v.2a1.9 1.9 0 0 1-1 1.6l-.4.2a1.9 1.9 0 0 1-1.9 0l-.2-.1a1.9 1.9 0 0 0-2.6.7l-.2.4a1.9 1.9 0 0 0 .7 2.6l.2.1a1.9 1.9 0 0 1 1 1.6v.6a1.9 1.9 0 0 1-1 1.6l-.2.1a1.9 1.9 0 0 0-.7 2.6l.2.4a1.9 1.9 0 0 0 2.6.7l.2-.1a1.9 1.9 0 0 1 1.9 0l.4.2a1.9 1.9 0 0 1 1 1.6v.2a1.9 1.9 0 0 0 1.9 1.9h.4a1.9 1.9 0 0 0 1.9-1.9v-.2a1.9 1.9 0 0 1 1-1.6l.4-.2a1.9 1.9 0 0 1 1.9 0l.2.1a1.9 1.9 0 0 0 2.6-.7l.2-.4a1.9 1.9 0 0 0-.7-2.6l-.2-.1a1.9 1.9 0 0 1-1-1.6v-.6a1.9 1.9 0 0 1 1-1.6l.2-.1a1.9 1.9 0 0 0 .7-2.6l-.2-.4a1.9 1.9 0 0 0-2.6-.7l-.2.1a1.9 1.9 0 0 1-1.9 0l-.4-.2a1.9 1.9 0 0 1-1-1.6v-.2a1.9 1.9 0 0 0-1.9-1.9z",
    },
    { circle: [12, 12, 3] },
  ],
  menu: [{ path: "M4 7h16M4 12h16M4 17h16" }],
  more: [
    { circle: [6, 12, 2.1], solid: true },
    { circle: [12, 12, 2.1], solid: true },
    { circle: [18, 12, 2.1], solid: true },
  ],
  "chevron-down": [{ path: "m6.5 9.5 5.5 5.5 5.5-5.5" }],
  "chevron-right": [{ path: "m9.5 6.5 5.5 5.5-5.5 5.5" }],
  "chevron-left": [{ path: "m14.5 6.5-5.5 5.5 5.5 5.5" }],
  back: [{ path: "M19 12H5" }, { path: "m10.5 17.5-5.5-5.5 5.5-5.5" }],
  "external-link": [{ path: "M14 5h5v5M19 5l-8 8M17 14v5H5V7h5" }],
  expand: [{ path: "M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5" }],
  collapse: [{ path: "M9 4v5H4M15 4v5h5M15 20v-5h5M9 20v-5H4" }],

  // ── Actions ───────────────────────────────────────────────────────
  plus: [{ path: "M12 5v14M5 12h14" }],
  close: [{ path: "M6.5 6.5l11 11M17.5 6.5l-11 11" }],
  check: [{ path: "m5 12.5 4.5 4.5L19 7.5" }],
  send: [{ path: "M12 19V5M6.5 10.5 12 5l5.5 5.5" }],
  stop: [{ rect: [7, 7, 10, 10, 1.8], solid: true }],
  play: [{ path: "M8 5.5v13l10.5-6.5z" }],
  pause: [{ path: "M9 6v12M15 6v12" }],
  reload: [{ path: "M5 12a7 7 0 1 0 2.2-5.1" }, { path: "M5 4.5V8h3.5" }],
  restore: [{ rect: [4, 13.5, 16, 6.5, 1.5] }, { path: "M12 14V4.5M8 8.5l4-4 4 4" }],
  edit: [{ path: "M5 19h3.5L19 8.5 15.5 5 5 15.5z" }],
  copy: [
    { rect: [7.5, 7.5, 12.5, 13, 1.5] },
    { path: "M7.5 16.5h-2A1.5 1.5 0 0 1 4 15V5.5A1.5 1.5 0 0 1 5.5 4H15a1.5 1.5 0 0 1 1.5 1.5v2" },
  ],
  paperclip: [
    {
      path: "m15.5 7.5-6.4 6.4a1.8 1.8 0 0 0 2.6 2.6l6.9-6.9a3.7 3.7 0 0 0-5.3-5.3l-7 7a5.6 5.6 0 0 0 8 8l6.2-6.2",
    },
  ],
  handoff: [{ path: "M4 12h12M12 7.5l4.5 4.5-4.5 4.5" }, { path: "M20 5v14" }],
  eye: [
    { path: "M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" },
    { circle: [12, 12, 2.5] },
  ],
  "eye-off": [
    { path: "M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" },
    { circle: [12, 12, 2.5] },
    { path: "m4 4 16 16" },
  ],

  // ── Things and states ─────────────────────────────────────────────
  warning: [{ path: "M12 4 21 19.5H3z" }, { path: "M12 10v4.5M12 17.2v.3" }],
  info: [{ circle: [12, 12, 8.5] }, { path: "M12 11v5.5M12 7.8v.3" }],
  layers: [{ path: "m12 4 8.5 4.5L12 13 3.5 8.5z" }, { path: "m3.5 13 8.5 4.5 8.5-4.5" }],
  sessions: [
    { path: "M4 6h10M7.5 12H20M5.5 18h10" },
    { circle: [18, 6, 1.5] },
    { circle: [3.75, 12, 1.5] },
    { circle: [19, 18, 1.5] },
  ],
  sliders: [
    { path: "M4 7h9M17 7h3M4 17h3M11 17h9" },
    { circle: [15, 7, 2] },
    { circle: [9, 17, 2] },
  ],
  hash: [{ path: "M9.5 4 8 20M16 4l-1.5 16M4.5 9h15M4 15h15" }],
  key: [{ circle: [8, 15, 3.5] }, { path: "m10.5 12.5 8-8M16 7l2.5 2.5" }],
  memory: [{ rect: [4, 6, 16, 12, 2] }, { path: "M8 10v4M12 10v4M16 10v4" }],
  bolt: [{ path: "M13 3 5 13.5h6L10 21l8-10.5h-6z" }],
  "wifi-off": [
    {
      path: "M4 9.5a12 12 0 0 1 5-2.7M14.5 6.9A12 12 0 0 1 20 9.5M7 13a7.5 7.5 0 0 1 3-1.6M10 16.5a3 3 0 0 1 4 0",
    },
    { path: "m4 4 16 16" },
  ],
  pulse: [{ path: "M4.5 14.5h3.5M10.25 9.5h3.5M16 14.5h3.5" }],
  bug: [
    { path: "M8.25 5.25 10 7.75M15.75 5.25 14 7.75" },
    { path: "M7 13.5a5 6.5 0 1 0 10 0a5 6.5 0 1 0-10 0z" },
    { path: "M12 8v11" },
    {
      path: "M7 11.5 4.75 10M17 11.5l2.25-1.5M7 14.5H4.5M17 14.5h2.5M7 17.5l-2.25 1.5M17 17.5l2.25 1.5",
    },
  ],
  spark: [
    { path: "M12 3v6M12 15v6M3 12h6M15 12h6" },
    { path: "m7.5 7.5 1.75 1.75M16.5 16.5l-1.75-1.75M16.5 7.5l-1.75 1.75M7.5 16.5l1.75-1.75" },
  ],
  mark: [{ path: "M12 2.5 21.5 12 12 21.5 2.5 12z" }, { path: "M8.6 12 12 8.6" }],
} as const satisfies Record<string, readonly IconShape[]>;

export type IconName = keyof typeof DRAWINGS;

export const ICONS: Readonly<Record<IconName, readonly IconShape[]>> = DRAWINGS;
