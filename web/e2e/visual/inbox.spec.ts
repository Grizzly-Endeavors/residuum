import type { Locator, Page } from "@playwright/test";
import { MAIL_SEED } from "../../mock/data/mail";
import { expect, test } from "../support/fixtures";
import { expectFileRendered } from "../support/lazy";
import { expectClipScreenshot, expectScreenshot } from "../support/screenshot";

/**
 * The Inbox's design reference: the list beside a Markdown thread with
 * attachments, a thread of replies with the reply box, New message, and a
 * thread with an attachment open beside it in the file panel. Each is shot
 * alone, on the desktop.
 */

const SYNC = MAIL_SEED.syncResearch;
const AUDIT = MAIL_SEED.wikiAudit;

/** The open thread. */
function thread(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation", exact: true });
}

test.describe("inbox", { tag: "@visual" }, () => {
  test.skip(
    ({ isMobile }) => isMobile,
    "The Inbox is shot at the desktop size, list beside thread.",
  );

  test("the list beside a Markdown thread with attachments", async ({ page }) => {
    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    await expect(thread(page).getByRole("heading", { name: SYNC.subject, level: 2 })).toBeVisible();
    await expect(thread(page).getByRole("button", { name: "Open findings.md" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Conversations", exact: true })).toBeVisible();
    await page.mouse.move(0, 0);
    await expectScreenshot(page.getByRole("main"), "inbox-thread");
  });

  test("a thread of replies, with your answer's seen marks and the reply box", async ({ page }) => {
    // Tall enough for the whole thread and its reply box, which the place otherwise scrolls.
    await page.setViewportSize({ width: page.viewportSize()?.width ?? 0, height: 1300 });
    await page.goto(`/inbox?thread=${AUDIT.threadId}`);
    await expect(thread(page).getByRole("article")).toHaveCount(4);
    await expect(thread(page).getByRole("form", { name: "Reply to the thread" })).toBeVisible();
    await page.mouse.move(0, 0);
    await expectScreenshot(thread(page), "inbox-replies");
  });

  test("New message, addressed to two agents, with a file", async ({ page }) => {
    await page.goto("/inbox?compose=1&to=atlas,scout");
    const compose = page.getByRole("form", { name: "New message" });
    await compose.getByRole("textbox", { name: "Subject" }).fill("Release checklist");
    await compose
      .getByRole("textbox", { name: /^Message/ })
      .fill(
        "Please **review** the checklist before Friday:\n\n- tag the release\n- publish the notes",
      );
    await page.getByLabel("Files to attach").setInputFiles({
      name: "checklist.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("1. Tag the release\n2. Publish the notes\n"),
    });
    await expect(compose.getByRole("button", { name: "Remove checklist.txt" })).toBeVisible();
    await expect(
      compose.getByRole("list", { name: "Attached files" }).getByRole("progressbar"),
    ).toHaveCount(0);
    // Focus leaves the message box, so its ring is not part of the shot.
    await compose.getByRole("textbox", { name: /^Message/ }).blur();
    await page.mouse.move(0, 0);
    await expectScreenshot(compose, "inbox-new-message");
  });

  test("an attachment open beside its thread in the file panel", async ({ page }) => {
    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    await thread(page).getByRole("button", { name: "Open findings.md" }).click();
    const panel = page.getByRole("complementary", { name: "findings.md" });
    await expect(panel).toBeVisible();
    await expectFileRendered(page, "Local-first sync: findings");
    await page.mouse.move(0, 0);
    await expectClipScreenshot([page.getByRole("main"), panel], "inbox-thread-panel");
  });
});
