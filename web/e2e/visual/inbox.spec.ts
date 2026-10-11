import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The Inbox's baselines: the list at 1440 wide and on the phone, an item open
 * with Markdown and an attachment, the list at a medium width, and the empty
 * archive of an agent.
 */

/** Open the Inbox at `url` and wait for its list to hold `items` items. */
async function openInbox(page: Page, url = "/inbox", items = 4): Promise<void> {
  await page.goto(url);
  const list = page.getByRole("region", { name: "Inbox" }).getByRole("list", { name: "Items" });
  await expect(list.locator(":scope > li")).toHaveCount(items);
}

test.describe("inbox", { tag: "@visual" }, () => {
  test("the list, at 1440 wide or on the phone", async ({ page }) => {
    await openInbox(page);
    await expectScreenshot(page, "inbox");
  });

  test("an item open, with Markdown and an attachment", async ({ page, mock }) => {
    const { id } = (await mock.post("/api/mock/user-inbox-add", {
      params: { agent: "scout" },
      data: {
        title: "Svelte 5.4 shipped",
        body: "Two changes matter for the web UI:\n\n- **Snippets** can be passed as props\n- `$state.raw` skips deep proxies\n\nRelease notes are attached.",
        attachments: [{ filename: "release-notes.txt" }],
      },
    })) as { id: string };
    await openInbox(page, `/inbox?item=scout:${id}`, 5);
    await expect(page.getByRole("link", { name: /release-notes\.txt/ })).toBeVisible();
    await expectScreenshot(page, "inbox-open");
  });

  test("at medium width", async ({ page, isMobile }) => {
    test.skip(isMobile, "Medium width is a desktop window narrowed.");
    await page.setViewportSize({ width: 1000, height: 800 });
    await openInbox(page);
    await expectScreenshot(page, "inbox-medium");
  });

  test("an agent's empty archive", async ({ page }) => {
    await page.goto("/inbox?agent=drifter&tab=archived");
    await expect(page.getByText("Nothing from drifter is archived.")).toBeVisible();
    await expectScreenshot(page, "inbox-empty");
  });
});
