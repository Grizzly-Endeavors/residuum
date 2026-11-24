import { agentPath } from "./paths";
import type { MessageAttachment } from "./types";

const PANEL_EXTENSIONS = [".md", ".txt"];

export function opensInPanel(attachment: Pick<MessageAttachment, "filename">): boolean {
  const name = attachment.filename.toLowerCase();
  return PANEL_EXTENSIONS.some((extension) => name.endsWith(extension));
}

export function attachmentDownloadPath(agent: string, itemId: string): string {
  return agentPath(agent, `/inbox/agent/${encodeURIComponent(itemId)}/attachments/0`);
}

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
