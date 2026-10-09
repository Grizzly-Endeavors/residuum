import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The baselines for a reply's Markdown: its headings, the checklist and the
 * code block at the end of the conversation, and the table that is wider than
 * a phone.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

/** Ask the mock for its Markdown showcase and wait for the end of it. */
async function showcase(page: Page): Promise<Locator> {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  const box = page.getByRole("textbox", { name: "Message atlas" });
  await box.fill("markdown please");
  await box.press("Enter");
  await expect(feed.getByText("Should parked notifications expire after a week")).toBeVisible();
  await expect(page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ })).toBeAttached();
  return feed;
}

test.describe("markdown reply", { tag: "@visual" }, () => {
  test("the end of a reply: headings, checklist, code and a question", async ({ page }) => {
    await showcase(page);
    await expectScreenshot(page, "prose-reply");
  });

  test("a table wider than the message, scrolling in its group", async ({ page }) => {
    const feed = await showcase(page);
    await feed.getByRole("group", { name: "Table" }).evaluate((group) => {
      group.scrollIntoView({ block: "center" });
    });
    await expectScreenshot(page, "prose-table", {
      mask: [page.getByRole("button", { name: "Jump to latest" })],
    });
  });
});
