// The words the Inbox shows for an item's source.

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
