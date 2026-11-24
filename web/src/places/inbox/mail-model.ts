import type { MailFolder, MailThread, MailThreadSummary } from "../../lib/hub-types";
import type { Panel } from "../../lib/routes";

export const FOLDERS: readonly { folder: MailFolder; label: string }[] = [
  { folder: "inbox", label: "Inbox" },
  { folder: "sent", label: "Sent" },
  { folder: "archived", label: "Archived" },
];

export function participantsLabel(summary: MailThreadSummary, folder: MailFolder): string {
  if (summary.participants.length === 0) return "You";
  const agents = summary.participants.join(", ");
  return folder === "sent" ? `To ${agents}` : agents;
}

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

export function panelOfThread(thread: MailThread | null, panel: Panel | null): boolean {
  if (thread === null || panel?.kind !== "file" || panel.agent === null) return false;
  const { agent, path } = panel;
  return thread.messages.some((message) =>
    message.attachments.some(
      (attachment) => attachment.agent === agent && attachment.path === path,
    ),
  );
}
