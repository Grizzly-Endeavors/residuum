import { expect, test } from "../support/fixtures";
import { expectSetupOpen } from "../support/lazy";
import { expectScreenshot } from "../support/screenshot";

/** The setup wizard's design reference: the "Add model providers" step. */

test.describe("setup wizard", { tag: "@visual" }, () => {
  test.skip(({ isMobile }) => isMobile, "The wizard is shot at the desktop size.");

  test("the model providers step", async ({ page, mock }) => {
    await mock.post("/api/mock/reset", { data: { setup: true } });
    await page.goto("/");
    await expectSetupOpen(page);
    await page.getByRole("button", { name: "Next" }).click();
    await page.getByRole("switch", { name: "OpenAI" }).click();
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Add model providers");
    await expectScreenshot(page.locator(".setup-column"), "setup");
  });
});
