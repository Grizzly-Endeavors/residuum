import { expect, test } from "../support/fixtures";

test("the workbench lists the team's artifacts", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Menu" })
      .click();
  }
  await page
    .getByRole("navigation", { name: "Places and agents" })
    .getByRole("link", { name: "Workbench" })
    .click();

  await expect(page).toHaveURL(/\/team\/workbench$/);
  await expect(page.getByRole("heading", { name: "Workbench", level: 1 })).toBeVisible();
  await expect(page.getByRole("link", { name: /^Tip Splitter/ })).toHaveAttribute(
    "href",
    "/team/workbench/tip-splitter",
  );
});
