import { expect, test } from "../support/fixtures";

test("Home's agent management lists every agent with its lifecycle controls", async ({ page }) => {
  await page.goto("/home");

  await page.getByRole("button", { name: "Start, stop, delete and add agents" }).click();
  const manage = page.getByRole("region", { name: "Manage agents" });
  await expect(manage).toBeVisible();
  for (const agent of ["atlas", "brittle", "drifter", "scout"]) {
    await expect(manage.getByRole("group", { name: `${agent} lifecycle` })).toBeVisible();
  }
  await expect(manage.getByRole("form", { name: "Create an agent" })).toBeVisible();
});

test("the root address opens Home", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveURL(/\/home$/);
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
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
