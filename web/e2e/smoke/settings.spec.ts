import { expect, test } from "../support/fixtures";

test("agent settings open from the menu with the agent's runtime values", async ({ page }) => {
  await page.goto("/agent/atlas");
  await page.getByRole("button", { name: "Menu" }).click();
  await page.getByRole("button", { name: "Agent settings" }).click();

  await expect(page).toHaveURL(/\/agent\/atlas\/settings\/runtime$/);
  await expect(page.getByRole("heading", { name: "atlas settings" })).toBeVisible();
  await expect(page.getByRole("spinbutton", { name: "Timeout (seconds)" })).toHaveValue("120");
});
