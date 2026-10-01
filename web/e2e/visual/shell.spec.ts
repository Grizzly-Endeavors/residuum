import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The shell's baselines. A hosted legacy view is painted over: its own unit
 * gives it baselines when it is rebuilt.
 */

async function shellScreenshot(page: Page, name: string): Promise<void> {
  await expectScreenshot(page, name, { mask: [page.locator("[data-legacy-view]")] });
}

test.describe("shell", { tag: "@visual" }, () => {
  test("on an agent's chat", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(
      page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
    ).toBeVisible();
    await shellScreenshot(page, "shell-chat");
  });

  test("on Home, with every agent closed", async ({ page }) => {
    await page.goto("/home");
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
    await shellScreenshot(page, "shell-home");
  });

  test("with the hub out of reach", async ({ page, mock }) => {
    await page.goto("/agent/atlas/activity");
    await expect(page.getByRole("heading", { name: "Sessions" })).toBeVisible();
    await mock.post("/api/mock/hub-socket", { data: { online: false } });
    await expect(page.getByText("Can't reach Residuum.")).toBeVisible();
    await shellScreenshot(page, "shell-offline");
  });

  test("the help menu", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas/files");
    await expect(page.getByRole("button", { name: /memory/ })).toBeVisible();
    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Menu" })
        .click();
    }
    await page
      .getByRole("navigation", { name: "Places and agents" })
      .getByRole("button", { name: "Help" })
      .click();
    await expect(page.getByRole("menu", { name: "Help" })).toBeVisible();
    // A mask paints over everything in its box, the drawer included, so the
    // phone's drawer shot leaves the dimmed place behind it unmasked.
    if (isMobile) await expectScreenshot(page, "shell-help-menu");
    else await shellScreenshot(page, "shell-help-menu");
  });

  test("at medium width", async ({ page, isMobile }) => {
    test.skip(isMobile, "Medium width is a desktop window narrowed.");
    await page.setViewportSize({ width: 1000, height: 760 });
    await page.goto("/agent/scout/schedule");
    await expect(page.getByText("Pulses", { exact: true })).toBeVisible();
    await shellScreenshot(page, "shell-medium");
  });
});
