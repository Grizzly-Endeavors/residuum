import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** The shell's design reference: the navigation rail, which a phone opens as a drawer from Menu. */

test.describe("shell", { tag: "@visual" }, () => {
  test("the navigation rail, or the drawer on a phone", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas/schedule");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Menu" })
        .click();
      const drawer = page.getByRole("dialog", { name: "Agents and places" });
      await expect(drawer.getByRole("navigation", { name: "Places and agents" })).toBeVisible();
      await expectScreenshot(drawer, "shell-nav");
    } else {
      const rail = page.getByRole("navigation", { name: "Places and agents" });
      await expect(rail).toBeVisible();
      await expectScreenshot(rail, "shell-nav");
    }
  });
});
