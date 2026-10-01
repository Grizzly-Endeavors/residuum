import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** Files and Shared files: the tree, the editor in the context panel, and a file's history. */

test.describe("files", { tag: "@visual" }, () => {
  test("an agent's files, with a file open and edited", async ({ page }) => {
    await page.goto("/agent/atlas/files?panel=file:config/channels.toml");
    const text = page.getByRole("textbox", { name: "Contents of channels.toml" });
    await expect(text).toHaveValue(/\[telegram\]/);
    await text.press("ControlOrMeta+End");
    await text.pressSequentially("broken = ");
    await expect(page.getByRole("list", { name: "Problems in channels.toml" })).toBeVisible();
    await text.blur();
    await expectScreenshot(page, "files-editor");
  });

  test("the shared files, with a folder open", async ({ page }) => {
    await page.goto("/team/files");
    await page.getByRole("button", { name: "wiki", exact: true }).click();
    await expect(page.locator('[data-path="wiki/projects"]')).toBeVisible();
    await expectScreenshot(page, "files-shared");
  });

  test("a file's history", async ({ page }) => {
    await page.goto("/agent/atlas/files?panel=file:SOUL.md");
    await page.getByRole("button", { name: "History" }).click();
    const dialog = page.getByRole("dialog", { name: "History of SOUL.md" });
    await expect(dialog.getByText("+- **Craft**", { exact: false })).toBeVisible();
    await expectScreenshot(page, "files-history");
  });
});
