import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The primitives gallery's design reference: the buttons, the fields with one
 * focused, an open menu, and the toasts. Each is shot alone, on the desktop.
 */

test.describe("primitives gallery", { tag: "@visual" }, () => {
  test.skip(({ isMobile }) => isMobile, "The gallery's controls are shot at the desktop size.");

  test.beforeEach(async ({ page }) => {
    await page.goto("/dev/gallery");
    await expect(page.getByRole("heading", { name: "Primitives", level: 1 })).toBeVisible();
  });

  function section(page: Page, name: string): Locator {
    return page.getByRole("region", { name, exact: true });
  }

  test("buttons", async ({ page }) => {
    await expectScreenshot(section(page, "Buttons"), "gallery-buttons");
  });

  test("text, number and select fields, one text field focused", async ({ page }) => {
    const fields = section(page, "Text, number and select");
    await fields.getByRole("textbox", { name: "Name", exact: true }).focus();
    await expectScreenshot(fields, "gallery-fields");
  });

  test("a menu, open with keyboard focus on an item", async ({ page }) => {
    const floating = section(page, "Menus, popovers and tooltips");
    await floating.getByRole("button", { name: "Manage atlas" }).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitem", { name: "Start", exact: true })).toBeFocused();
    await expectScreenshot(page.getByRole("menu"), "gallery-menu");
  });

  test("an error toast and a toast with Undo", async ({ page }) => {
    const toasts = section(page, "Toasts and recent notifications");
    // Errors stay and an action keeps a toast up for 10 seconds, so these hold still.
    await toasts.getByRole("button", { name: "Error" }).click();
    await toasts.getByRole("button", { name: "With Undo" }).click();
    await expect(page.getByText("Couldn't save the settings.")).toBeVisible();
    await expect(page.getByText("Removed the Brave Search key.")).toBeVisible();
    await expectScreenshot(
      page.getByRole("region", { name: "Notifications", exact: true }),
      "toasts",
    );
  });
});
