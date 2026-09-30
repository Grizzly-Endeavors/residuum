// What the router writes into `history.state` for each entry it creates. The
// marks answer two questions the browser can't: "did this page push the entry
// that opened the panel or modal?" (closing is then `history.back()`), and "is
// this entry an overlay's?" (Back then closes the overlay and nothing else).

import type { AppLocation } from "./routes";

export interface EntryState {
  /** Position among the entries this page lineage pushed; each push is one more than the entry it follows. */
  idx: number;
  /** The `idx` of the entry this page pushed to open the Settings modal, while the modal is open. */
  settings?: number;
  /** The same, for the context panel. */
  panel?: number;
  /** Set on an entry pushed for an overlay: same URL as the entry below it. */
  overlay?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined;
}

/** The marks on a history entry, or null for an entry this router didn't write. */
export function readEntry(state: unknown): EntryState | null {
  if (!isRecord(state) || typeof state.idx !== "number") return null;
  const entry: EntryState = { idx: state.idx };
  const settings = optionalNumber(state.settings);
  const panel = optionalNumber(state.panel);
  if (settings !== undefined) entry.settings = settings;
  if (panel !== undefined) entry.panel = panel;
  if (typeof state.overlay === "string") entry.overlay = state.overlay;
  return entry;
}

/**
 * The marks for a new entry pushed after `entry`, which showed `from`. A
 * parameter that appears with this push was opened by it; one that was already
 * open keeps the entry that opened it, or none when the page didn't push that.
 */
export function entryAfterPush(entry: EntryState, from: AppLocation, to: AppLocation): EntryState {
  const idx = entry.idx + 1;
  const next: EntryState = { idx };
  if (to.settings !== null) {
    if (from.settings === null) next.settings = idx;
    else if (entry.settings !== undefined) next.settings = entry.settings;
  }
  if (to.panel !== null) {
    if (from.panel === null) next.panel = idx;
    else if (entry.panel !== undefined) next.panel = entry.panel;
  }
  return next;
}

/**
 * The marks for `entry` after its location changes by replace: the parameters
 * that stay open keep their opener, and one that appears was not pushed by
 * this page.
 */
export function entryAfterReplace(
  entry: EntryState,
  from: AppLocation,
  to: AppLocation,
): EntryState {
  const next: EntryState = { idx: entry.idx };
  if (to.settings !== null && from.settings !== null && entry.settings !== undefined) {
    next.settings = entry.settings;
  }
  if (to.panel !== null && from.panel !== null && entry.panel !== undefined) {
    next.panel = entry.panel;
  }
  if (entry.overlay !== undefined) next.overlay = entry.overlay;
  return next;
}

/** Whether two entries carry the same marks. */
export function sameEntry(a: EntryState | null, b: EntryState): boolean {
  return (
    a !== null &&
    a.idx === b.idx &&
    a.settings === b.settings &&
    a.panel === b.panel &&
    a.overlay === b.overlay
  );
}

/** The same entry without the overlay mark, for an entry whose overlay is gone. */
export function withoutOverlay(entry: EntryState): EntryState {
  const { overlay: _overlay, ...rest } = entry;
  return rest;
}

/** The entry to push for an overlay opened on `entry`. */
export function overlayEntryAfter(entry: EntryState, id: string): EntryState {
  return { ...entry, idx: entry.idx + 1, overlay: id };
}

/**
 * How many entries back closing a parameter goes, when this page pushed the
 * entry that opened it; null when it didn't, so closing replaces instead.
 */
export function stepsToClose(entry: EntryState, opener: number | undefined): number | null {
  return opener === undefined ? null : entry.idx - opener + 1;
}
