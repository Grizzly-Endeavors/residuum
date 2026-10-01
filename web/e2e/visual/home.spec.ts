import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * Home's baselines: at 1440 and 1920 wide, at a medium width where the right
 * column follows the board, and on the phone, where the board is cards.
 */

async function openHome(page: Page): Promise<void> {
  await page.goto("/home");
  await expect(page.getByRole("region", { name: /^Needs you/ }).getByRole("listitem")).toHaveCount(
    5,
  );
  await expect(
    page.getByRole("region", { name: "Across the team" }).getByRole("listitem"),
  ).toHaveCount(4);
  await expect(
    page.getByRole("region", { name: "Coming up" }).getByRole("listitem"),
  ).not.toHaveCount(0);
}

test.describe("home", { tag: "@visual" }, () => {
  test("at 1440 wide, or on the phone", async ({ page }) => {
    await openHome(page);
    await expectScreenshot(page, "home");
  });

  test("at 1920 wide, centered", async ({ page, isMobile }) => {
    test.skip(isMobile, "A wide window is a desktop size.");
    await page.setViewportSize({ width: 1920, height: 1080 });
    await openHome(page);
    await expectScreenshot(page, "home-1920");
  });

  test("at medium width, the right column under the board", async ({ page, isMobile }) => {
    test.skip(isMobile, "Medium width is a desktop window narrowed.");
    await page.setViewportSize({ width: 1000, height: 1100 });
    await openHome(page);
    await expectScreenshot(page, "home-medium");
  });

  test("the board as cards on the phone", async ({ page, isMobile }) => {
    test.skip(!isMobile, "The board is a table at wider widths.");
    await openHome(page);
    await page.getByRole("table", { name: "Agents" }).scrollIntoViewIfNeeded();
    await expectScreenshot(page, "home-board-phone");
  });

  test("the Create agent dialog, a sheet on the phone", async ({ page }) => {
    await openHome(page);
    await page.getByRole("button", { name: "New agent" }).click();
    const dialog = page.getByRole("dialog", { name: "Create an agent" });
    await dialog.getByRole("textbox", { name: "Name" }).fill("Research");
    await dialog
      .getByRole("textbox", { name: "What should it help with?" })
      .fill("Keep my reading list and remind me what I haven't finished");
    await dialog.getByRole("button", { name: "More options" }).click();
    await expect(dialog.getByRole("combobox", { name: "Who can find it" })).toBeVisible();
    await expectScreenshot(page, "home-create-agent");
  });

  test("an agent's menu, open on its row", async ({ page }) => {
    await openHome(page);
    await page.getByRole("button", { name: "Manage atlas" }).click();
    await expect(page.getByRole("menu", { name: "Manage atlas" })).toBeVisible();
    await expectScreenshot(page, "home-agent-menu");
  });

  test("Recently deleted, open under the board", async ({ page }) => {
    await openHome(page);
    const removed = await page.request.delete("/api/hub/agents/drifter");
    expect(removed.ok()).toBe(true);
    const deleted = page.getByRole("status").filter({ hasText: "You deleted drifter." });
    await deleted.getByRole("button", { name: "Dismiss notification" }).click();
    await expect(deleted).toBeHidden();

    await page.getByRole("button", { name: "Recently deleted" }).click();
    const restore = page.getByRole("button", { name: "Restore drifter" });
    await restore.scrollIntoViewIfNeeded();
    await expect(restore).toBeVisible();
    await expectScreenshot(page, "home-recently-deleted");
  });
});
