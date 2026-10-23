// Words and choices the open thread and the composers share: how a list of
// agents reads, which attachments open beside the thread, the lines that say
// what a file may weigh and when the recipients will see what is sent, and
// the key that sends.

import { UPLOAD_LIMIT_LABEL } from "../../lib/api";
import type { MailAttachment, MailSeen } from "../../lib/hub-types";

/** Names as a sentence lists them: "atlas", "atlas and scout", "atlas, scout and wren". */
export function joinNames(names: readonly string[], word = "and"): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} ${word} ${names[names.length - 1] ?? ""}`;
}

/** Extensions the file panel shows as text, so the attachment opens beside the thread. */
const PANEL_EXTENSIONS = [".md", ".txt"];

/** The attachment opens in the file panel rather than downloading. */
export function opensInPanel(attachment: Pick<MailAttachment, "filename">): boolean {
  const name = attachment.filename.toLowerCase();
  return PANEL_EXTENSIONS.some((extension) => name.endsWith(extension));
}

/** The thread's line under its subject: who is in it and how many messages it holds. */
export function threadMeta(participants: readonly string[], messageCount: number): string {
  const who = participants.length === 0 ? "You" : `You and ${participants.join(", ")}`;
  return `${who} · ${messageCount === 1 ? "1 message" : `${String(messageCount)} messages`}`;
}

/** What one file may weigh, as the composers say before anyone is refused. */
export function uploadLimitHint(): string {
  return `Any file, up to ${UPLOAD_LIMIT_LABEL} each.`;
}

/**
 * Agents read their mail when they next check their inbox, not when it is
 * sent; the composer says so, naming the one recipient when there is one.
 */
export function deliveryHint(recipients: readonly string[]): string {
  const [only] = recipients;
  if (recipients.length === 1 && only !== undefined) {
    return `${only} will see this next time it checks its inbox.`;
  }
  return "They’ll see this next time they check their inbox.";
}

/** What a New message says in place of the delivery hint until it has a recipient. */
export const PICK_AGENT_HINT = "Pick at least one agent.";

/** Ctrl or Cmd with Enter sends from a composer's text box, as mail apps do. */
export function sendOnModEnter(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "preventDefault">,
  send: () => void,
): void {
  if (event.key !== "Enter" || !(event.ctrlKey || event.metaKey)) return;
  event.preventDefault();
  send();
}

/** Who has seen a message of yours, and who hasn't yet, in one line. */
export function seenLine(seen: readonly MailSeen[]): string {
  const yes = seen.filter((mark) => mark.seen).map((mark) => mark.agent);
  const no = seen.filter((mark) => !mark.seen).map((mark) => mark.agent);
  if (no.length === 0) return `Seen by ${joinNames(yes)}`;
  if (yes.length === 0) {
    const [only] = no;
    return no.length === 1 && only !== undefined
      ? `Not seen yet · waiting in ${only}’s inbox`
      : `Not seen yet by ${joinNames(no, "or")}`;
  }
  return `Seen by ${joinNames(yes)}, not yet by ${joinNames(no, "or")}`;
}
