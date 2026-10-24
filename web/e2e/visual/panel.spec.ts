import { expect, test } from "../support/fixtures";
import { expectFileRendered } from "../support/lazy";
import { expectScreenshot } from "../support/screenshot";

/** The context panel's design reference: a file open beside the chat. */

test.describe("context panel", { tag: "@visual" }, () => {
  test.skip(({ isMobile }) => isMobile, "The panel is shot beside the chat, at the desktop size.");

  test("a file beside the chat", async ({ page }) => {
    await page.goto("/agent/atlas?panel=file:team/wiki/index.md");
    await expect(page.getByRole("heading", { name: "index.md" })).toBeVisible();
    await expectFileRendered(page, "Wiki Index");
    await expectScreenshot(page.getByRole("complementary", { name: "index.md" }), "panel-file");
  });
});
