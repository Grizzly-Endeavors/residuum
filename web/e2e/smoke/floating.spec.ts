import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * Menus, popovers, tooltips, toasts and the Recent notifications dialog,
 * driven from the primitives gallery.
 */

async function openGallery(page: Page): Promise<void> {
  await page.goto("/dev/gallery");
  await expect(page.getByRole("heading", { name: "Primitives", level: 1 })).toBeVisible();
}

function floatingSection(page: Page): Locator {
  return page.getByRole("region", { name: "Menus, popovers and tooltips" });
}

function menuButton(page: Page): Locator {
  return floatingSection(page).getByRole("button", { name: "Manage atlas" });
}

test.describe("menus", { tag: "@dev" }, () => {
  test("a menu opens from its button, and a press outside closes it", async ({ page }) => {
    await openGallery(page);
    await menuButton(page).click();
    const menu = page.getByRole("menu", { name: "Manage atlas" });
    await expect(menu).toBeVisible();
    await expect(menuButton(page)).toHaveAttribute("aria-expanded", "true");
    await expectNoAxeViolations(page);

    await page.getByRole("heading", { name: "Primitives", level: 1 }).click();
    await expect(menu).toBeHidden();
    await expect(menuButton(page)).toHaveAttribute("aria-expanded", "false");
  });

  test("the keyboard opens it, moves, types ahead, chooses and closes it", async ({ page }) => {
    await openGallery(page);
    await menuButton(page).focus();
    await page.keyboard.press("Enter");
    const menu = page.getByRole("menu", { name: "Manage atlas" });
    await expect(menu.getByRole("menuitem", { name: "Open chat" })).toBeFocused();

    await page.keyboard.press("ArrowUp");
    await expect(menu.getByRole("menuitem", { name: "Delete atlas" })).toBeFocused();
    await page.keyboard.press("Home");
    await page.keyboard.press("r");
    await expect(menu.getByRole("menuitem", { name: "Restart" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(menu).toBeHidden();
    await expect(menuButton(page)).toBeFocused();

    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("s");
    await page.keyboard.press("s");
    await expect(menu.getByRole("menuitem", { name: "Stop" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(menu).toBeHidden();
    await expect(page.getByText("Stopped atlas.")).toBeVisible();
    await expect(menuButton(page)).toBeFocused();
  });

  test("a checkbox item keeps the menu open, and Tab closes it onto its button", async ({
    page,
  }) => {
    await openGallery(page);
    await menuButton(page).click();
    const autostart = page.getByRole("menuitemcheckbox", { name: "Start automatically" });
    await expect(autostart).toHaveAttribute("aria-checked", "true");
    await autostart.click();
    await expect(autostart).toHaveAttribute("aria-checked", "false");

    await page.keyboard.press("Tab");
    await expect(page.getByRole("menu")).toBeHidden();
    await expect(menuButton(page)).toBeFocused();
  });

  test("a menu item opens the Recent notifications dialog over the page", async ({ page }) => {
    await openGallery(page);
    const help = floatingSection(page).getByRole("button", { name: "Help" });
    await help.click();
    await page.getByRole("menuitem", { name: "Recent notifications" }).click();
    const dialog = page.getByRole("dialog", { name: "Recent notifications" });
    await expect(dialog).toBeVisible();
    await expect(page.getByRole("menu")).toBeHidden();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(help).toBeFocused();
  });
});

test.describe("popovers", { tag: "@dev" }, () => {
  test("a popover opens on its first control, and Esc closes it onto its button", async ({
    page,
  }) => {
    await openGallery(page);
    const button = floatingSection(page).getByRole("button", { name: "claude-9" });
    await button.click();
    const popover = page.getByRole("dialog", { name: "Model for atlas" });
    await expect(popover).toBeVisible();
    await expect(popover.getByRole("combobox", { name: "Model, from Anthropic" })).toBeFocused();
    await expectNoAxeViolations(page);

    await page.keyboard.press("Escape");
    await expect(popover).toBeHidden();
    await expect(button).toBeFocused();

    await button.click();
    await page.getByRole("heading", { name: "Primitives", level: 1 }).click();
    await expect(popover).toBeHidden();
  });

  test("it stays beside its button inside the viewport", async ({ page }) => {
    await openGallery(page);
    const button = floatingSection(page).getByRole("button", { name: "claude-9" });
    await button.click();
    const popover = page.getByRole("dialog", { name: "Model for atlas" });
    const box = await popover.boundingBox();
    const anchor = await button.boundingBox();
    const viewport = page.viewportSize();
    if (box === null || anchor === null || viewport === null) throw new Error("nothing to measure");
    expect(box.x).toBeGreaterThanOrEqual(8);
    expect(box.x + box.width).toBeLessThanOrEqual(viewport.width - 8);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(viewport.height);
    // Below its button, or flipped above it when there is no room below.
    const below = box.y >= anchor.y + anchor.height;
    const above = box.y + box.height <= anchor.y;
    expect(below || above).toBe(true);
  });
});

test.describe("tooltips", { tag: "@dev" }, () => {
  test("keyboard focus shows a tooltip, and moving on hides it", async ({ page }) => {
    await openGallery(page);
    const section = floatingSection(page);
    await section.getByRole("button", { name: "Settings" }).focus();
    await page.keyboard.press("Tab");
    const tooltip = page.getByRole("tooltip");
    await expect(tooltip).toHaveText("Copy the agent's address");
    await expect(section.getByRole("button", { name: "Copy" })).toHaveAccessibleDescription(
      "Copy the agent's address",
    );
    await expectNoAxeViolations(page);

    await page.keyboard.press("Escape");
    await expect(tooltip).toBeHidden();
    await expect(section.getByRole("button", { name: "Copy" })).toBeFocused();
  });

  test("a pointer resting on an icon button shows its label", async ({ page }) => {
    await openGallery(page);
    const section = floatingSection(page);
    await section.getByRole("button", { name: "Settings" }).hover();
    await expect(page.getByRole("tooltip")).toHaveText("Settings");
    await page.getByRole("heading", { name: "Primitives", level: 1 }).hover();
    await expect(page.getByRole("tooltip")).toBeHidden();
  });
});

test.describe("toasts", { tag: "@dev" }, () => {
  test("toasts show, errors stay until dismissed, and an action runs once", async ({ page }) => {
    await openGallery(page);
    const section = page.getByRole("region", { name: "Toasts and recent notifications" });
    await section.getByRole("button", { name: "Error" }).click();
    await section.getByRole("button", { name: "With Undo" }).click();
    const errors = page.getByRole("alert").filter({ hasText: "Couldn't save the settings." });
    const others = page.getByRole("status").filter({ hasText: "Removed the Brave Search key." });
    await expect(errors).toContainText("Couldn't save the settings.");
    await expect(others).toBeVisible();
    await expectNoAxeViolations(page);

    await others.getByRole("button", { name: "Undo" }).click();
    await expect(page.getByText("Removed the Brave Search key.")).toBeHidden();
    await expect(page.getByText("Put the Brave Search key back.")).toBeVisible();

    await errors.getByRole("button", { name: "Dismiss notification" }).click();
    await expect(page.getByText("Couldn't save the settings.")).toBeHidden();
  });
});

test.describe("recent notifications", { tag: "@dev" }, () => {
  async function openWithSamples(page: Page): Promise<Locator> {
    await openGallery(page);
    const section = page.getByRole("region", { name: "Toasts and recent notifications" });
    await section.getByRole("button", { name: "Add sample notifications" }).click();
    await section.getByRole("button", { name: "Recent notifications" }).click();
    const dialog = page.getByRole("dialog", { name: "Recent notifications" });
    await expect(dialog).toBeVisible();
    return dialog;
  }

  test("lists what was surfaced, newest first, with details", async ({ page }) => {
    const dialog = await openWithSamples(page);
    const items = dialog.getByRole("listitem");
    await expect(items).toHaveCount(3);
    await expect(items.first()).toContainText("atlas couldn't reach Anthropic.");
    await expect(items.first()).toContainText("just now");
    await items.first().getByRole("button", { name: "Details" }).click();
    await expect(dialog.getByText("provider anthropic answered 401 Unauthorized")).toBeVisible();
    await expectNoAxeViolations(page);

    await page.goBack();
    await expect(dialog).toBeHidden();
  });

  test("clears, and Undo in the dialog brings them back", async ({ page }) => {
    const dialog = await openWithSamples(page);
    await dialog.getByRole("button", { name: "Clear all" }).click();
    await expect(dialog.getByRole("listitem")).toHaveCount(0);
    await expect(dialog).toContainText("Cleared 3 notifications.");
    const undo = dialog.getByRole("button", { name: "Undo" });
    await expect(undo).toBeFocused();
    await expectNoAxeViolations(page);

    await undo.click();
    await expect(dialog.getByRole("listitem")).toHaveCount(3);
    await expect(dialog.getByRole("button", { name: "Clear all" })).toBeFocused();
  });

  test("clears, and the toast's Undo above the dialog brings them back", async ({ page }) => {
    const dialog = await openWithSamples(page);
    await dialog.getByRole("button", { name: "Clear all" }).click();
    const toast = page.getByRole("status").filter({ hasText: "Cleared 3 notifications." });
    await toast.getByRole("button", { name: "Undo" }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("listitem")).toHaveCount(3);
    await expect(dialog.getByRole("button", { name: "Undo" })).toBeHidden();
  });
});
