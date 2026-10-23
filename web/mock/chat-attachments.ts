import type { MessageAttachment } from "../src/lib/generated/protocol";
import { agentInboxItemId } from "./agent-inbox";
import { attachmentPath, storeAttachment } from "./inbox";
import type { MockAgent, MockHub } from "./state";

/** What a chat message's attached files became: each saved file, and the text that names them. */
export interface SavedChatAttachments {
  attachments: MessageAttachment[];
  /** The message text with a line naming each saved file, as the agent reads it. */
  content: string;
}

/** What the backend says when a chat message names an upload that isn't staged. */
export const UPLOAD_GONE = "An attached file isn't available anymore. Attach it again, then send.";

/**
 * Save the uploads a chat message names (`send_message`'s `attachments`) as
 * the backend does: each becomes an item of its own in the agent's inbox
 * (source `chat`, titled `Chat attachment: <name>`) holding the file, and the
 * message text gains a line naming it. Every id is resolved before anything is
 * saved, so one that isn't staged saves nothing and returns `null`. The used
 * uploads are taken off the hub.
 */
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
  const lines: string[] = [];
  for (const upload of staged) {
    if (upload === undefined) continue;
    const title = `Chat attachment: ${upload.filename}`;
    const id = agentInboxItemId([agent.state], title);
    const stored = storeAttachment(agent.state, "agent", id, upload.filename, upload.bytes);
    agent.state.agentInbox.push({
      id,
      title,
      body: "",
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
    content:
      lines.length === 0 ? content : [content, ...lines].filter((line) => line !== "").join("\n"),
  };
}
