import type { Locator, Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const ASKED = "Check the memory settings";
const NOTE = "Looking through recent notes first.";
const REPLY_END = "Would you like me to adjust any of these values?";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

test("a page reloaded while a reply streams shows the whole turn so far", async ({
  page,
  mock,
}) => {
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
  await expect(feed.getByText(/^Working(?: \d+s)?$/)).toBeVisible();
  await expect(feed.getByText(/before this page connected/)).toHaveCount(0);

  await mock.post("/api/mock/turn-hold", { data: { held: false } });
  await expect(feed.getByText(/^Working/)).toHaveCount(0);
  await expect(feed.getByText(REPLY_END)).toHaveCount(1);
  await expect(feed.getByText(ASKED, { exact: true })).toHaveCount(1);
});
