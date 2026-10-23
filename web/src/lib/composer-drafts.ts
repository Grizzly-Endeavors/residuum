// What the user has typed to each agent and not sent yet. The text is kept in
// local storage, so it survives navigation and reload; attached files are
// kept for the life of the page only, since a file can't be put in storage,
// and their uploads carry on while the chat shows another agent.

import { StagedAttachments, type StagedAttachment } from "./staged-attachments.svelte";

const STORAGE_PREFIX = "residuum-draft:";

const attachments = new Map<string, StagedAttachments>();

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

/** The files attached to `agent`'s draft on this page: the same list each time it is asked for. */
export function draftAttachments(agent: string): StagedAttachments {
  let staged = attachments.get(agent);
  if (staged === undefined) {
    staged = new StagedAttachments();
    attachments.set(agent, staged);
  }
  return staged;
}

/**
 * Put a message that didn't go back in `agent`'s draft, for when no composer
 * is showing: its text ahead of anything typed since, its files ahead of any
 * attached since.
 */
export function restoreDraft(
  agent: string,
  content: string,
  files: readonly StagedAttachment[],
): void {
  if (content !== "") {
    const typed = readDraft(agent);
    saveDraft(agent, typed.trim() === "" ? content : `${content}\n\n${typed}`);
  }
  draftAttachments(agent).restore(files);
}

/** Throw away every agent's attached files, cancelling uploads still running. */
export function clearDraftAttachments(): void {
  for (const staged of attachments.values()) staged.clear();
  attachments.clear();
}
