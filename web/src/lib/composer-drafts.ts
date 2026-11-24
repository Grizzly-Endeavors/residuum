import { StagedAttachments, type StagedAttachment } from "./staged-attachments.svelte";

const STORAGE_PREFIX = "residuum-draft:";

const attachments = new Map<string, StagedAttachments>();

export function readDraft(agent: string): string {
  try {
    return localStorage.getItem(STORAGE_PREFIX + agent) ?? "";
  } catch {
    return "";
  }
}

export function saveDraft(agent: string, text: string): void {
  try {
    if (text === "") localStorage.removeItem(STORAGE_PREFIX + agent);
    else localStorage.setItem(STORAGE_PREFIX + agent, text);
  } catch {}
}

export function draftAttachments(agent: string): StagedAttachments {
  let staged = attachments.get(agent);
  if (staged === undefined) {
    staged = new StagedAttachments();
    attachments.set(agent, staged);
  }
  return staged;
}

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

export function clearDraftAttachments(): void {
  for (const staged of attachments.values()) staged.clear();
  attachments.clear();
}
