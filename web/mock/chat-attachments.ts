import type { ImageAttachment, MessageAttachment } from "../src/lib/generated/protocol";
import { agentInboxItemId } from "./agent-inbox";
import { attachmentPath, mimeTypeOf, storeAttachment } from "./inbox";
import type { MockAgent, MockHub } from "./state";

export interface SavedChatAttachments {
  attachments: MessageAttachment[];
  images: ImageAttachment[];
  content: string;
}

const INLINE_IMAGE_TYPES: ReadonlySet<string> = new Set([
  "image/jpeg",
  "image/png",
  "image/gif",
  "image/webp",
]);
const MAX_INLINE_IMAGE_BYTES = 20 * 1024 * 1024;

/** What the backend says when a chat message names an upload that isn't staged. */
export const UPLOAD_GONE =
  "An attached file wasn't available anymore, so it's uploading again. Send once it's ready.";

export function saveChatAttachments(
  hub: MockHub,
  agent: MockAgent,
  content: string,
  ids: readonly string[],
): SavedChatAttachments | null {
  const uploads = hub.hubState.uploads;
  const staged = ids.map((id) => uploads.get(id));
  if (staged.some((upload) => upload === undefined)) return null;
  const attachments: MessageAttachment[] = [];
  const images: ImageAttachment[] = [];
  const lines: string[] = [];
  for (const upload of staged) {
    if (upload === undefined) continue;
    const title = `Chat attachment: ${upload.filename}`;
    const id = agentInboxItemId([agent.state], title);
    const stored = storeAttachment(agent.state, "agent", id, upload.filename, upload.bytes);
    const mimeType = mimeTypeOf(upload.filename);
    if (INLINE_IMAGE_TYPES.has(mimeType) && upload.bytes.length <= MAX_INLINE_IMAGE_BYTES) {
      images.push({ media_type: mimeType, data: upload.bytes.toString("base64") });
    }
    agent.state.agentInbox.push({
      id,
      title,
      body: `From: Web chat\nSize: ${String(upload.bytes.length)} bytes\nContent-Type: ${stored.mime_type}`,
      source: "chat",
      timestamp: hub.env.clock.iso(),
      attachments: [stored],
    });
    const path = attachmentPath("agent", id, upload.filename, false);
    attachments.push({
      item_id: id,
      filename: upload.filename,
      size: upload.bytes.length,
      content_type: stored.mime_type,
      path,
    });
    lines.push(`[Attachment: ${upload.filename} (${String(upload.bytes.length)} bytes) → ${path}]`);
    uploads.delete(upload.id);
  }
  return {
    attachments,
    images,
    content:
      lines.length === 0 ? content : [content, ...lines].filter((line) => line !== "").join("\n"),
  };
}
