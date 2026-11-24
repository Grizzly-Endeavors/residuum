import { UPLOAD_LIMIT_LABEL } from "../../lib/api";
import type { MailSeen } from "../../lib/hub-types";
import { joinNames } from "../../lib/words";

export function threadMeta(participants: readonly string[], messageCount: number): string {
  const who = participants.length === 0 ? "You" : `You and ${participants.join(", ")}`;
  return `${who} · ${messageCount === 1 ? "1 message" : `${String(messageCount)} messages`}`;
}

export function uploadLimitHint(): string {
  return `Any file, up to ${UPLOAD_LIMIT_LABEL} each.`;
}

export function deliveryHint(recipients: readonly string[]): string {
  const [only] = recipients;
  if (recipients.length === 1 && only !== undefined) {
    return `${only} will see this next time it checks its inbox.`;
  }
  return "They’ll see this next time they check their inbox.";
}

export const PICK_AGENT_HINT = "Pick at least one agent.";

export function sendOnModEnter(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "preventDefault">,
  send: () => void,
): void {
  if (event.key !== "Enter" || !(event.ctrlKey || event.metaKey)) return;
  event.preventDefault();
  send();
}

export function moveOnEnter(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "isComposing" | "preventDefault">,
  next: () => void,
): void {
  if (event.key !== "Enter" || event.isComposing || event.ctrlKey || event.metaKey) return;
  event.preventDefault();
  next();
}

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
