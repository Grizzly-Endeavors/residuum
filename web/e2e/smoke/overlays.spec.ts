import type { Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The modal overlays (Dialog, Sheet, Drawer and the confirm dialog), driven
 * from the primitives gallery: each opens on its own history entry, so Back
 * closes it and leaves the page where it was.
 */

const GALLERY = "/dev/gallery";

async function openGallery(page: Page): Promise<void> {
  await page.goto(GALLERY);
  await expect(page.getByRole("heading", { name: "Primitives", level: 1 })).toBeVisible();
}

test.describe("modal overlays in the gallery", { tag: "@dev" }, () => {
  test("the gallery itself passes the accessibility scan", async ({ page }) => {
    await openGallery(page);
    await expectNoAxeViolations(page);
  });

  test("a dialog opens on its first field, and Back closes it", async ({ page }) => {
    await openGallery(page);
    const opener = page.getByRole("button", { name: "Create agent" });
    await opener.click();
    const dialog = page.getByRole("dialog", { name: "Create an agent" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("textbox", { name: "Name" })).toBeFocused();
    await expectNoAxeViolations(page);

    await page.goBack();
    await expect(dialog).toBeHidden();
    await expect(page).toHaveURL(GALLERY);
    await expect(opener).toBeFocused();
  });

  test("a sheet opens from the bottom, and Back closes it", async ({ page }) => {
    await openGallery(page);
    await page.getByRole("button", { name: "Switch agent" }).click();
    const sheet = page.getByRole("dialog", { name: "Switch agent" });
    await expect(sheet).toBeVisible();
    await expectNoAxeViolations(page);

    await page.goBack();
    await expect(sheet).toBeHidden();
    await expect(page).toHaveURL(GALLERY);
  });

  test("a drawer opens from the left, and Back closes it", async ({ page }) => {
    await openGallery(page);
    await page.getByRole("button", { name: "Agents and places" }).click();
    const drawer = page.getByRole("dialog", { name: "Agents and places" });
    await expect(drawer).toBeVisible();
    await expectNoAxeViolations(page);

    await page.goBack();
    await expect(drawer).toBeHidden();
    await expect(page).toHaveURL(GALLERY);
  });

  test("a confirm dialog starts on the safer choice, and Back counts as cancel", async ({
    page,
  }) => {
    await openGallery(page);
    await page.getByRole("button", { name: "Delete brittle" }).click();
    const confirm = page.getByRole("alertdialog", { name: "Delete brittle?" });
    await expect(confirm).toBeVisible();
    await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
    await expectNoAxeViolations(page);

    await page.goBack();
    await expect(confirm).toBeHidden();
    await expect(page.getByText("Kept brittle.")).toBeVisible();
  });

  test("a dialog opened from a dialog closes first on Back", async ({ page }) => {
    await openGallery(page);
    await page.getByRole("button", { name: "Edit instructions" }).click();
    const outer = page.getByRole("dialog", { name: "scout's instructions" });
    await outer.getByRole("button", { name: "Discard changes" }).click();
    const inner = page.getByRole("alertdialog", { name: "Discard your changes?" });
    await expect(inner).toBeVisible();

    await page.goBack();
    await expect(inner).toBeHidden();
    await expect(outer).toBeVisible();
    await page.goBack();
    await expect(outer).toBeHidden();
  });
});
