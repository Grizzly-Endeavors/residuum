import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";

/**
 * Picks a section from the settings section list. A phone keeps the list
 * collapsed behind a toggle, which a wider window doesn't show.
 */
async function pickSection(page: Page, name: string): Promise<void> {
  const sections = page.getByRole("navigation", { name: "Settings sections" });
  const toggle = sections.getByRole("button", { expanded: false });
  if (await toggle.isVisible()) await toggle.click();
  await sections.getByRole("button", { name, exact: true }).click();
}

test("agent settings open on the model section, and the runtime section shows the agent's runtime values", async ({
  page,
}) => {
  await page.goto("/agent/atlas");
  await page.getByRole("button", { name: "Menu" }).click();
  await page.getByRole("button", { name: "Agent settings" }).click();

  // The modal is part of the URL, and a URL that names no section opens the agent's Model section.
  await expect(page).toHaveURL(/\/agent\/atlas\?settings=atlas$/);
  await expect(page.getByRole("heading", { name: "atlas settings" })).toBeVisible();
  await expect(page.getByText("Provider Connections")).toBeVisible();

  await pickSection(page, "Runtime");

  await expect(page).toHaveURL(/\/agent\/atlas\?settings=atlas\/runtime$/);
  await expect(page.getByRole("heading", { name: "atlas settings" })).toBeVisible();
  await expect(page.getByRole("spinbutton", { name: "Timeout (seconds)" })).toHaveValue("120");
});
