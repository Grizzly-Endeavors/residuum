import type { Place } from "./routes";

export const APP_NAME = "Residuum";

export const NOT_FOUND_TITLE = "Page not found";

export interface TabTitleInput {
  subject: string | null;
  hidden: boolean;
  unread: number;
  working: boolean;
  finished: boolean;
}

export function tabTitle({ subject, hidden, unread, working, finished }: TabTitleInput): string {
  const count = hidden && unread > 0 ? `(${String(unread)}) ` : "";
  if (subject === null) return `${count}${APP_NAME}`;
  return `${count}${subject}${hiddenStatus({ hidden, working, finished })} · ${APP_NAME}`;
}

function hiddenStatus({
  hidden,
  working,
  finished,
}: Pick<TabTitleInput, "hidden" | "working" | "finished">): string {
  if (!hidden) return "";
  if (working) return " is working";
  return finished ? " finished" : "";
}

export function placeSubject(place: Place, agentName: (agent: string) => string): string | null {
  switch (place.kind) {
    case "home":
      return null;
    case "inbox":
      return "Inbox";
    case "workbench":
      return "Workbench";
    case "shared-files":
      return "Shared files";
    case "not-found":
      return NOT_FOUND_TITLE;
    case "chat":
    case "activity":
    case "schedule":
    case "files":
      return agentName(place.agent);
  }
}
