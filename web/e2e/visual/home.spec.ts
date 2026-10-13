import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** Home's design reference: the board (cards on a phone), what needs the reader, the team's activity and what is coming up. */

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
    // The block that holds the board and the team's column, which scrolls inside the page: shot whole, not as far as the window shows.
    const content = page.getByRole("group", { name: "Home overview" });
    await expectScreenshot(content, "home");
  });
});
