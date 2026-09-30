import type { UserInboxItem } from "../../src/lib/types";

/** The sample inbox: one unread item and one read item. */
export function createInboxItems(): UserInboxItem[] {
  return [
    {
      id: "mock_1",
      title: "Deploy tomorrow",
      body: "Reminder to trigger the deployment pipeline tomorrow morning.",
      source: "agent:pulse",
      timestamp: new Date().toISOString(),
      read: false,
      attachments: [],
    },
    {
      id: "mock_2",
      title: "Daily Digest",
      body: "Here is your daily summary.",
      source: "agent:digest",
      timestamp: new Date(Date.now() - 3600000).toISOString(),
      read: true,
      attachments: [],
    },
  ];
}
