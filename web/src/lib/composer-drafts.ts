// What the user has typed to each agent and not sent yet. The text is kept in
// local storage, so it survives navigation and reload; attached images are
// kept for the life of the page only, since a few of them would fill the
// storage the browser allows.

import type { ImageAttachment } from "./types";

const STORAGE_PREFIX = "residuum-draft:";

const images = new Map<string, ImageAttachment[]>();

/** The text typed to `agent` and not sent, or "" when there is none or storage is blocked. */
export function readDraft(agent: string): string {
  try {
    return localStorage.getItem(STORAGE_PREFIX + agent) ?? "";
  } catch {
    // Storage is blocked (a private window, site data off): the draft starts empty.
    return "";
  }
}

/** Keep `text` as the draft for `agent`; empty text forgets it. */
export function saveDraft(agent: string, text: string): void {
  try {
    if (text === "") localStorage.removeItem(STORAGE_PREFIX + agent);
    else localStorage.setItem(STORAGE_PREFIX + agent, text);
  } catch {
    // Storage is blocked or full: the draft holds while the composer is open.
  }
}

/** The images attached to `agent`'s draft on this page. */
export function readDraftImages(agent: string): ImageAttachment[] {
  return images.get(agent) ?? [];
}

export function saveDraftImages(agent: string, attached: readonly ImageAttachment[]): void {
  if (attached.length === 0) images.delete(agent);
  else images.set(agent, [...attached]);
}
