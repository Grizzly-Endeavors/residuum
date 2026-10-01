import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The context panel's baselines: beside the chat at wide width, floating at
 * medium width, and a full-screen sheet on phones, with a file, a session run
 * and the conversation size in it.
 */

/**
 * The chat beside the panel has read its model, and the hub has brought the
 * inbox count. On a phone the sheet makes both inert, which hides them from
 * role queries.
 */
async function chatSettled(page: Page): Promise<void> {
  const includeHidden = true;
  await expect(
    page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/, includeHidden }),
  ).toBeAttached();
  await expect(
    page.getByRole("link", { name: /^Inbox.*\d+ unread/, includeHidden }).first(),
  ).toBeAttached();
}

test.describe("context panel", { tag: "@visual" }, () => {
  test("a file beside the chat, or over it on a phone", async ({ page }) => {
    await page.goto("/agent/atlas?panel=file:team/wiki/index.md");
    await expect(page.getByRole("heading", { name: "index.md" })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Contents of index.md" })).toHaveValue(
      /# Wiki Index/,
    );
    await chatSettled(page);
    await expectScreenshot(page, "panel-file");
  });

  test("a session run beside Activity, or over it on a phone", async ({ page }) => {
    await page.goto("/agent/atlas/activity?panel=session:atlas:run-live-research");
    await expect(page.getByText("Starting with what's already in the wiki.")).toBeVisible();
    await expect(page.getByRole("heading", { name: /^Running now/ })).toBeAttached();
    await expectScreenshot(page, "panel-session");
  });

  test("the conversation size beside the chat, or over it on a phone", async ({ page }) => {
    await page.goto("/agent/atlas?panel=size");
    await expect(page.getByText("Tools used", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Token counts" }).click();
    await expect(page.getByText("412,880")).toBeVisible();
    await chatSettled(page);
    await expectScreenshot(page, "panel-size");
  });

  test("at medium width", async ({ page, isMobile }) => {
    test.skip(isMobile, "Medium width is a desktop window narrowed.");
    await page.setViewportSize({ width: 1000, height: 760 });
    await page.goto("/agent/scout/schedule?panel=file:SOUL.md");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Contents of SOUL.md" })).toHaveValue(/# Soul/);
    await expectScreenshot(page, "panel-medium");
  });
});
