import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * Baselines of the primitives gallery: every control at rest, then each modal
 * overlay and floating layer open over it.
 */

async function openGallery(page: Page): Promise<void> {
  await page.goto("/dev/gallery");
  await expect(page.getByRole("heading", { name: "Primitives", level: 1 })).toBeVisible();
}

/** Scroll a section to the top of the window, so what it opens has room around it. */
async function showSection(page: Page, name: string): Promise<Locator> {
  const section = page.getByRole("region", { name });
  await section.evaluate((element) => {
    element.scrollIntoView({ block: "start" });
  });
  return section;
}

async function dismissToasts(page: Page): Promise<void> {
  const dismiss = page.getByRole("button", { name: "Dismiss notification" });
  while ((await dismiss.count()) > 0) await dismiss.first().click();
}

test.describe("primitives gallery", { tag: "@visual" }, () => {
  test("every control", async ({ page }) => {
    await openGallery(page);
    await expectScreenshot(page, "gallery", { fullPage: true });
  });

  test("modal overlays", async ({ page }) => {
    await openGallery(page);
    const section = await showSection(page, "Dialogs, sheets and drawers");

    await section.getByRole("button", { name: "Create agent" }).click();
    await expect(page.getByRole("dialog", { name: "Create an agent" })).toBeVisible();
    await expectScreenshot(page, "gallery-dialog");
    await page.keyboard.press("Escape");

    await section.getByRole("button", { name: "Switch agent" }).click();
    await expect(page.getByRole("dialog", { name: "Switch agent" })).toBeVisible();
    await expectScreenshot(page, "gallery-sheet");
    await page.keyboard.press("Escape");

    await section.getByRole("button", { name: "Agents and places" }).click();
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeVisible();
    await expectScreenshot(page, "gallery-drawer");
    await page.keyboard.press("Escape");

    await section.getByRole("button", { name: "Delete brittle" }).click();
    await expect(page.getByRole("alertdialog", { name: "Delete brittle?" })).toBeVisible();
    await expectScreenshot(page, "gallery-confirm");
  });

  test("menus, popovers and tooltips", async ({ page }) => {
    await openGallery(page);
    const section = await showSection(page, "Menus, popovers and tooltips");

    await section.getByRole("button", { name: "Manage atlas" }).focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitem", { name: "Start", exact: true })).toBeFocused();
    await expectScreenshot(page, "gallery-menu");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toBeHidden();

    await section.getByRole("button", { name: "claude-9" }).click();
    await expect(page.getByRole("dialog", { name: "Model for atlas" })).toBeVisible();
    await expectScreenshot(page, "gallery-popover");
    await page.keyboard.press("Escape");

    await section.getByRole("button", { name: "Settings" }).focus();
    await page.keyboard.press("Tab");
    await expect(page.getByRole("tooltip")).toHaveText("Copy the agent's address");
    await expectScreenshot(page, "gallery-tooltip");
  });

  test("toasts and recent notifications", async ({ page }) => {
    await openGallery(page);
    const section = await showSection(page, "Toasts and recent notifications");

    // Errors stay and an action keeps a toast up for 10 seconds, so these hold still.
    await section.getByRole("button", { name: "Error" }).click();
    await section.getByRole("button", { name: "With Undo" }).click();
    await expect(page.getByText("Couldn't save the settings.")).toBeVisible();
    await expectScreenshot(page, "gallery-toasts");
    await dismissToasts(page);

    await section.getByRole("button", { name: "Add sample notifications" }).click();
    await dismissToasts(page);
    await section.getByRole("button", { name: "Recent notifications" }).click();
    const dialog = page.getByRole("dialog", { name: "Recent notifications" });
    await dialog.getByRole("button", { name: "Details" }).click();
    await expect(dialog.getByText("provider anthropic answered 401 Unauthorized")).toBeVisible();
    await expectScreenshot(page, "gallery-recent");
  });
});
