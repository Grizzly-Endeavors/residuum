import { expect, test } from "../support/fixtures";
import { expectSettingsOpen } from "../support/lazy";
import { expectScreenshot } from "../support/screenshot";

/** The Settings modal's design reference: a section with a staged change and the Unsaved changes bar. */

test.describe("settings modal", { tag: "@visual" }, () => {
  test("a section with a staged change and the save bar", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/runtime");
    const dialog = await expectSettingsOpen(page);
    await page.getByLabel("Reply time limit").fill("60");
    await expect(page.getByRole("region", { name: "Unsaved changes" })).toBeVisible();
    await page.getByLabel("Reply time limit").blur();
    await expectScreenshot(dialog, "settings-section");
  });
});
