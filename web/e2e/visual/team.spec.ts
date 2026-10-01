import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

test.describe("team page", { tag: "@visual" }, () => {
  test("lists every agent with its lifecycle controls", async ({ page }) => {
    await page.goto("/home");
    await expect(page.getByRole("region", { name: "Team" })).toBeVisible();
    await expect(page.getByText("Residuum connected")).toBeVisible();

    await expectScreenshot(page, "team");
  });
});
