import { afterEach, describe, expect, it, vi } from "vitest";
import { userInbox } from "./inbox.svelte";
import { notifications } from "./notifications.svelte";
import type { UserInboxItem } from "./types";

const ITEM: UserInboxItem = {
  id: "item-1",
  title: "Weekly report",
  body: "Here it is.",
  source: "agent",
  timestamp: "2026-09-26T14:00:00Z",
  read: true,
  attachments: [],
};

function respond(handler: (url: string) => Response): void {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => Promise.resolve(handler(input))),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  userInbox.items = [];
  userInbox.archivedItems = [];
  notifications.history = [];
});

describe("user inbox restore", () => {
  it("moves a restored item back to the inbox", async () => {
    userInbox.archivedItems = [ITEM];
    respond((url) =>
      url.endsWith("/restore")
        ? new Response(null, { status: 204 })
        : new Response(JSON.stringify([ITEM]), { status: 200 }),
    );

    await userInbox.restore("item-1");

    expect(userInbox.archivedItems).toEqual([]);
    expect(userInbox.items.map((i) => i.id)).toEqual(["item-1"]);
  });

  it("tells the user when a restore fails and keeps the item archived", async () => {
    userInbox.archivedItems = [ITEM];
    respond(() => new Response("failed to restore", { status: 500 }));

    await userInbox.restore("item-1");

    expect(userInbox.archivedItems).toHaveLength(1);
    expect(notifications.history[0]?.message).toContain("Couldn't restore that item.");
  });
});
