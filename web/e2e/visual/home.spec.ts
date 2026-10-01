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
});
