import type { UserInboxItem } from "../../src/lib/types";
import type { MockClock } from "../env";

/** The sample inbox: one unread item and one read item. */
export function createInboxItems(clock: MockClock): UserInboxItem[] {
  return [
    {
      id: "mock_1",
      title: "Deploy tomorrow",
      body: "Reminder to trigger the deployment pipeline tomorrow morning.",
      source: "agent:pulse",
      timestamp: clock.iso(),
      read: false,
      attachments: [],
    },
    {
      id: "mock_2",
      title: "Daily Digest",
      body: "Here is your daily summary.",
      source: "agent:digest",
      timestamp: clock.isoAgo(3_600_000),
      read: true,
      attachments: [],
    },
  ];
}

/** The sample archive: one item the user archived a week ago. */
export function createArchivedInboxItems(clock: MockClock): UserInboxItem[] {
  return [
    {
      id: "mock_archived_1",
      title: "Last week's digest",
      body: "Here was last week's summary.",
      source: "agent:digest",
      timestamp: clock.isoAgo(7 * 86_400_000),
      read: true,
      attachments: [],
    },
  ];
}
