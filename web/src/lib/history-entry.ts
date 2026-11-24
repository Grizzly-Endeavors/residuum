// What the router writes into `history.state` for each entry it creates. The
// marks answer two questions the browser can't: "did this page push the entry
// that opened the panel, the modal, the open inbox thread or New message, or
// the selected workbench artifact?" (closing is then
// `history.back()`), and "is this entry an overlay's?" (Back then closes the
// overlay and nothing else).

import { formatLocation, placesEqual, type AppLocation, type Place } from "./routes";

export interface EntryState {
  idx: number;
  settings?: number;
  section?: number;
  panel?: number;
  item?: number;
  backUrl?: string;
  overlay?: string;
}

export function panelAddress(location: AppLocation): string {
  return formatLocation({ ...location, settings: null });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined;
}

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
  if (typeof state.backUrl === "string") entry.backUrl = state.backUrl;
  if (typeof state.overlay === "string") entry.overlay = state.overlay;
  return entry;
}

function sameList(a: Place, b: Place): boolean {
  if (a.kind === "workbench") return b.kind === "workbench";
  return a.kind === "inbox" && b.kind === "inbox" && a.agent === b.agent && a.folder === b.folder;
}

function hasItem(location: AppLocation): boolean {
  const { place } = location;
  if (place.kind === "workbench") return place.artifact !== null;
  return place.kind === "inbox" && (place.thread !== null || place.compose !== null);
}

export function entryAfterPush(entry: EntryState, from: AppLocation, to: AppLocation): EntryState {
  const idx = entry.idx + 1;
  const next: EntryState = { idx };
  if (hasItem(to) && sameList(from.place, to.place)) {
    if (!hasItem(from)) next.item = idx;
    else if (entry.item !== undefined) next.item = entry.item;
  }
  if (!placesEqual(from.place, to.place)) return next;
  if (to.settings !== null) {
    if (from.settings === null) next.settings = idx;
    else if (entry.settings !== undefined) next.settings = entry.settings;
  }
  if (to.settings?.section != null && from.settings?.scope === to.settings.scope) {
    if (from.settings.section === null) next.section = idx;
    else if (entry.section !== undefined) next.section = entry.section;
  }
  if (to.panel !== null) {
    if (from.panel === null) next.panel = idx;
    else if (entry.panel !== undefined) next.panel = entry.panel;
  }
  if (from.panel !== null && to.panel !== null) {
    const replaced = panelAddress(from);
    if (replaced !== panelAddress(to)) next.backUrl = replaced;
  }
  return next;
}

export function entryAfterReplace(
  entry: EntryState,
  from: AppLocation,
  to: AppLocation,
): EntryState {
  const next: EntryState = { idx: entry.idx };
  if (entry.overlay !== undefined) next.overlay = entry.overlay;
  if (hasItem(to) && hasItem(from) && sameList(from.place, to.place)) {
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
  if (to.panel !== null && from.panel !== null && entry.backUrl !== undefined) {
    next.backUrl = entry.backUrl;
  }
  return next;
}

export function sameEntry(a: EntryState | null, b: EntryState): boolean {
  return (
    a !== null &&
    a.idx === b.idx &&
    a.settings === b.settings &&
    a.section === b.section &&
    a.panel === b.panel &&
    a.backUrl === b.backUrl &&
    a.item === b.item &&
    a.overlay === b.overlay
  );
}

export function withoutOverlay(entry: EntryState): EntryState {
  const { overlay: _overlay, ...rest } = entry;
  return rest;
}

export function overlayEntryAfter(entry: EntryState, id: string): EntryState {
  return { ...entry, idx: entry.idx + 1, overlay: id };
}

export function stepsToClose(entry: EntryState, opener: number | undefined): number | null {
  return opener === undefined ? null : entry.idx - opener + 1;
}
