// What the router writes into `history.state` for each entry it creates. The
// marks answer two questions the browser can't: "did this page push the entry
// that opened the panel, the modal or the open inbox item?" (closing is then
// `history.back()`), and "is this entry an overlay's?" (Back then closes the
// overlay and nothing else).

import { placesEqual, type AppLocation, type Place } from "./routes";

export interface EntryState {
  /** Position among the entries this page lineage pushed; each push is one more than the entry it follows. */
  idx: number;
  /** The `idx` of the entry this page pushed to open the Settings modal, while the modal is open. */
  settings?: number;
  /** The same, for a settings section opened from the phone's section list, while it is open. */
  section?: number;
  /** The same, for the context panel. */
  panel?: number;
  /** The same, for the item open in the Inbox. */
  item?: number;
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
  const section = optionalNumber(state.section);
  const panel = optionalNumber(state.panel);
  const item = optionalNumber(state.item);
  if (settings !== undefined) entry.settings = settings;
  if (section !== undefined) entry.section = section;
  if (panel !== undefined) entry.panel = panel;
  if (item !== undefined) entry.item = item;
  if (typeof state.overlay === "string") entry.overlay = state.overlay;
  return entry;
}

/** Whether both places are the Inbox with the same filter and tab, whatever item is open. */
function sameInboxList(a: Place, b: Place): boolean {
  return a.kind === "inbox" && b.kind === "inbox" && a.agent === b.agent && a.tab === b.tab;
}

/** Whether the location is the Inbox with an item open. */
function hasItem(location: AppLocation): boolean {
  return location.place.kind === "inbox" && location.place.item !== null;
}

/**
 * The marks for a new entry pushed after `entry`, which showed `from`. A
 * parameter that appears with this push was opened by it; one that was already
 * open keeps the entry that opened it, or none when the page didn't push that.
 * The open inbox item counts as one while the list around it stays the same.
 * A push that also changes the place has no other marks: going back would
 * leave the place, and closing a parameter only removes it.
 */
export function entryAfterPush(entry: EntryState, from: AppLocation, to: AppLocation): EntryState {
  const idx = entry.idx + 1;
  const next: EntryState = { idx };
  if (hasItem(to) && sameInboxList(from.place, to.place)) {
    if (!hasItem(from)) next.item = idx;
    else if (entry.item !== undefined) next.item = entry.item;
  }
  if (!placesEqual(from.place, to.place)) return next;
  if (to.settings !== null) {
    if (from.settings === null) next.settings = idx;
    else if (entry.settings !== undefined) next.settings = entry.settings;
  }
  // A section's opener is the scope's section list, so it holds while the scope does.
  if (to.settings?.section != null && from.settings?.scope === to.settings.scope) {
    if (from.settings.section === null) next.section = idx;
    else if (entry.section !== undefined) next.section = entry.section;
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
 * this page. A replace that changes the place drops them, for the same reason
 * as a push that does.
 */
export function entryAfterReplace(
  entry: EntryState,
  from: AppLocation,
  to: AppLocation,
): EntryState {
  const next: EntryState = { idx: entry.idx };
  if (entry.overlay !== undefined) next.overlay = entry.overlay;
  if (hasItem(to) && hasItem(from) && sameInboxList(from.place, to.place)) {
    if (entry.item !== undefined) next.item = entry.item;
  }
  if (!placesEqual(from.place, to.place)) return next;
  if (to.settings !== null && from.settings !== null && entry.settings !== undefined) {
    next.settings = entry.settings;
  }
  const sameScope = from.settings?.scope === to.settings?.scope;
  if (to.settings?.section != null && from.settings?.section != null && sameScope) {
    if (entry.section !== undefined) next.section = entry.section;
  }
  if (to.panel !== null && from.panel !== null && entry.panel !== undefined) {
    next.panel = entry.panel;
  }
  return next;
}

/** Whether two entries carry the same marks. */
export function sameEntry(a: EntryState | null, b: EntryState): boolean {
  return (
    a !== null &&
    a.idx === b.idx &&
    a.settings === b.settings &&
    a.section === b.section &&
    a.panel === b.panel &&
    a.item === b.item &&
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
