import type { Locator, Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

/**
 * A page that opens partway through a turn shows everything the turn has
 * done so far, its reply still streaming, and nothing of it twice.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const ASKED = "Check the memory settings";
const NOTE = "Looking through recent notes first.";
/** The end of the reply the turn streams. */
const REPLY_END = "Would you like me to adjust any of these values?";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

test("a page reloaded while a reply streams shows the whole turn so far", async ({
  page,
  mock,
}) => {
  // The turn writes its reply, then waits before it ends.
  await mock.post("/api/mock/turn-hold", { data: { held: "reply" } });
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  const box = page.getByRole("combobox", { name: "Message atlas" });
  await box.fill(ASKED);
  await sendFromComposer(box);
  await expect(feed.getByText(REPLY_END)).toBeVisible();

  await page.reload();
  await expect(feed.getByText(GREETING)).toBeVisible();
  await expect(feed.getByText(ASKED, { exact: true })).toHaveCount(1);
  await expect(feed.getByText(NOTE)).toHaveCount(1);
  await expect(feed.getByText(REPLY_END)).toBeVisible();
  await expect(feed.getByRole("button", { name: "Searched memory, read 2 files" })).toBeVisible();
  // Timed from when the turn started, not from when the page opened, nor on the server's clock.
  await expect(feed.getByText(/^Working(?: \d+s)?$/)).toBeVisible();
  // Nothing is missing, so the turn doesn't say steps happened before the page connected.
  await expect(feed.getByText(/before this page connected/)).toHaveCount(0);

  await mock.post("/api/mock/turn-hold", { data: { held: false } });
  await expect(feed.getByText(/^Working/)).toHaveCount(0);
  await expect(feed.getByText(REPLY_END)).toHaveCount(1);
  await expect(feed.getByText(ASKED, { exact: true })).toHaveCount(1);
});
