import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

test.describe("home", { tag: "@visual" }, () => {
  test("the board, what needs you, the team's activity and what is coming up", async ({ page }) => {
    await page.goto("/home");
    await expect(
      page.getByRole("region", { name: /^Needs you/ }).getByRole("listitem"),
    ).toHaveCount(5);
    await expect(
      page.getByRole("region", { name: "Across the team" }).getByRole("listitem"),
    ).toHaveCount(4);
    await expect(
      page.getByRole("region", { name: "Coming up" }).getByRole("listitem"),
    ).not.toHaveCount(0);
    const content = page.getByRole("main").locator("[data-home-overview]");
    await expectScreenshot(content, "home");
  });
});
