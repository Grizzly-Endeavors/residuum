// The Inbox place's words and rules that don't need the page: who a row is
// with, what an empty list says, and which file panel belongs to a thread.

import type { MailFolder, MailThread, MailThreadSummary } from "../../lib/hub-types";
import type { Panel } from "../../lib/routes";

/** The folders in the order the place lists them, with their names. */
export const FOLDERS: readonly { folder: MailFolder; label: string }[] = [
  { folder: "inbox", label: "Inbox" },
  { folder: "sent", label: "Sent" },
  { folder: "archived", label: "Archived" },
];

/**
 * Who a thread is with, as its row names them: the agents, in the order they
 * joined. In Sent the row reads "To atlas, wren", since the mail went to them.
 */
export function participantsLabel(summary: MailThreadSummary, folder: MailFolder): string {
  if (summary.participants.length === 0) return "You";
  const agents = summary.participants.join(", ");
  return folder === "sent" ? `To ${agents}` : agents;
}

/** What a list with no threads says, for its folder and agent filter. */
export function emptyListText(folder: MailFolder, agent: string | null): string {
  switch (folder) {
    case "inbox":
      return agent === null
        ? "You're all caught up. Mail from your agents lands here."
        : `Nothing from ${agent} in your inbox.`;
    case "sent":
      return agent === null
        ? "You haven't sent anything yet. Write to one agent or several with New message."
        : `You haven't written to ${agent} yet.`;
    case "archived":
      return agent === null
        ? "Nothing is archived. Archived conversations wait here until you move them back."
        : `Nothing with ${agent} is archived.`;
  }
}

/**
 * Whether `panel` shows one of `thread`'s attachments: an agent's file at the
 * path an attachment names. Such a panel belongs to the thread and closes with
 * it, as a mail app's preview closes with the conversation.
 */
export function panelOfThread(thread: MailThread | null, panel: Panel | null): boolean {
  if (thread === null || panel?.kind !== "file" || panel.agent === null) return false;
  const { agent, path } = panel;
  return thread.messages.some((message) =>
    message.attachments.some(
      (attachment) => attachment.agent === agent && attachment.path === path,
    ),
  );
}
