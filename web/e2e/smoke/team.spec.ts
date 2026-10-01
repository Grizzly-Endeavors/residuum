import { expect, test } from "../support/fixtures";

test("Home hosts the team page, listing every agent with its lifecycle controls", async ({
  page,
}) => {
  await page.goto("/home");

  const team = page.getByRole("region", { name: "Team" });
  await expect(team).toBeVisible();
  for (const agent of ["atlas", "brittle", "drifter", "scout"]) {
    await expect(team.getByRole("group", { name: `${agent} lifecycle` })).toBeVisible();
  }
  await expect(team.getByRole("form", { name: "Create an agent" })).toBeVisible();
});

test("the root address opens Home", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveURL(/\/home$/);
  await expect(page.getByRole("region", { name: "Team" })).toBeVisible();
});

test("the rail's + leads to creating an agent", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Menu" })
      .click();
  }
  await page
    .getByRole("navigation", { name: "Places and agents" })
    .getByRole("button", { name: "Create an agent" })
    .click();

  await expect(page).toHaveURL(/\/home$/);
  await expect(page.getByRole("textbox", { name: "Name" })).toBeFocused();
});
