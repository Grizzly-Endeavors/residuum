import type { Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

async function setVisibility(page: Page, state: "visible" | "hidden"): Promise<void> {
  await page.evaluate((value) => {
    Object.defineProperty(document, "visibilityState", { value, configurable: true });
    document.dispatchEvent(new Event("visibilitychange"));
  }, state);
}

test("the tab is titled with the agent or place the page is on", async ({ page }) => {
  await page.goto("/agent/atlas");
  await expect(page).toHaveTitle("atlas · Residuum");

  await page.goto("/home");
  await expect(page).toHaveTitle("Residuum");

  await page.goto("/inbox");
  await expect(page).toHaveTitle("Inbox · Residuum");

  await page.goto("/agent/scout/files");
  await expect(page).toHaveTitle("scout · Residuum");
});

test("a hidden tab says the agent is working, then that it finished", async ({ page, mock }) => {
  await page.goto("/agent/atlas");
  await mock.post("/api/mock/turn-hold", { data: { held: true } });
  const composer = page.getByRole("combobox", { name: "Message atlas" });
  await composer.fill("Look into the wiki");
  await sendFromComposer(composer);

  await setVisibility(page, "hidden");
  await expect(page).toHaveTitle("(2) atlas is working · Residuum");

  await mock.post("/api/mock/turn-hold", { data: { held: false } });
  await expect(page).toHaveTitle("(2) atlas finished · Residuum");

  await setVisibility(page, "visible");
  await expect(page).toHaveTitle("atlas · Residuum");
});
