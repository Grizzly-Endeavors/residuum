// The words the Inbox shows for an item's source and its attachments' sizes.

/** What a source's kind (the part before `:`) means to the user. */
const SOURCE_KINDS: Readonly<Record<string, string>> = {
  hub: "Residuum",
  cli: "Command line",
  pulse: "Regular check",
  action: "Scheduled action",
  cron: "Scheduled task",
  artifact: "Workbench page",
  webhook: "Webhook",
};

/**
 * Where an item came from, in words, or null when its agent sent it itself
 * (`agent`), which the agent's name already says. A source with a detail
 * (`pulse:Inbox check`, `agent:digest`) keeps it.
 */
export function sourceLabel(source: string): string | null {
  const colon = source.indexOf(":");
  const kind = colon < 0 ? source : source.slice(0, colon);
  const detail = colon < 0 ? "" : source.slice(colon + 1).trim();
  if (kind === "agent") return detail === "" ? null : detail;
  const word = SOURCE_KINDS[kind] ?? kind;
  return detail === "" ? word : `${word}: ${detail}`;
}

/** A file size as people read it: "812 B", "4.2 KB", "1.3 MB". */
export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${String(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
