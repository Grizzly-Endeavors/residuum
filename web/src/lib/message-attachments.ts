// What a message's attachments do in the conversation: which open beside the
// thread in the Files panel, where the others download from, and which
// `[Attachment: …]` text lines the attachment chips stand in for.

import { agentPath } from "./paths";
import type { MessageAttachment } from "./types";

/** Extensions the file panel shows as text, so the attachment opens beside the thread. */
const PANEL_EXTENSIONS = [".md", ".txt"];

/** The attachment opens in the file panel rather than downloading. */
export function opensInPanel(attachment: Pick<MessageAttachment, "filename">): boolean {
  const name = attachment.filename.toLowerCase();
  return PANEL_EXTENSIONS.some((extension) => name.endsWith(extension));
}

/** Where a saved attachment's bytes download from, served whether its agent runs or not. */
export function attachmentDownloadPath(agent: string, itemId: string): string {
  return agentPath(agent, `/inbox/agent/${encodeURIComponent(itemId)}/attachments/0`);
}

/**
 * The message text without the `[Attachment: …]` lines its attachments stand
 * for. Each attachment takes one line, matched by its name and by the arrow
 * that comes before its path; a line that says the download failed has an
 * em dash instead and stays. Other text is kept as it is.
 */
export function withoutAttachmentLines(
  content: string,
  attachments: readonly Pick<MessageAttachment, "filename">[],
): string {
  if (attachments.length === 0) return content;
  const lines = content.split("\n");
  const taken = new Set<number>();
  for (const attachment of attachments) {
    const head = `[Attachment: ${attachment.filename} (`;
    const index = lines.findIndex(
      (line, at) =>
        !taken.has(at) && line.startsWith(head) && line.includes(" → ") && line.endsWith("]"),
    );
    if (index !== -1) taken.add(index);
  }
  if (taken.size === 0) return content;
  const kept = lines.filter((_, at) => !taken.has(at));
  return kept.join("\n").trimEnd();
}
