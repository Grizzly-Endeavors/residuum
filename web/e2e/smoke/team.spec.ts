import { expect, test } from "../support/fixtures";

test("the team page lists every agent with its lifecycle controls", async ({ page }) => {
  await page.goto("/agent/atlas");
  await page
    .getByRole("navigation", { name: "Agents" })
    .getByRole("button", { name: "Team" })
    .click();

  await expect(page).toHaveURL(/\/home$/);
  const team = page.getByRole("region", { name: "Team" });
  await expect(team).toBeVisible();
  for (const agent of ["atlas", "brittle", "drifter", "scout"]) {
    await expect(team.getByRole("group", { name: `${agent} lifecycle` })).toBeVisible();
  }
  await expect(team.getByRole("form", { name: "Create an agent" })).toBeVisible();
});

test("the root address opens the team page at /home", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveURL(/\/home$/);
  await expect(page.getByRole("region", { name: "Team" })).toBeVisible();
});
