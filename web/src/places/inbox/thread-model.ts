// Words and choices the open thread and the composers share: how a list of
// agents reads, which attachments open beside the thread, and the line that
// says when the recipients will see what is sent.

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

/**
 * Agents read their mail when they next check their inbox, not when it is
 * sent; the composer says so, naming the one recipient when there is one.
 */
export function deliveryHint(recipients: readonly string[]): string {
  const [only] = recipients;
  if (recipients.length === 1 && only !== undefined) {
    return `${only} sees it next time it checks its inbox.`;
  }
  return "Each sees it next time it checks its inbox.";
}

/** Who has seen a message of yours, and who hasn't yet, in one line. */
export function seenLine(seen: readonly MailSeen[]): string {
  const yes = seen.filter((mark) => mark.seen).map((mark) => mark.agent);
  const no = seen.filter((mark) => !mark.seen).map((mark) => mark.agent);
  if (no.length === 0) return `Seen by ${joinNames(yes)}`;
  if (yes.length === 0) return `Not seen yet by ${joinNames(no, "or")}`;
  return `Seen by ${joinNames(yes)}, not yet by ${joinNames(no, "or")}`;
}
