import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectSetupOpen } from "../support/lazy";
import { expectScreenshot } from "../support/screenshot";

/**
 * The wizard scrolls inside its own region, so a full-page screenshot shows
 * only what fits the window. Grow the window to the step's height instead, so
 * each baseline holds the whole step.
 */
async function showWholeStep(page: Page): Promise<void> {
  const width = page.viewportSize()?.width ?? 0;
  const height = await page.locator("main").evaluate((main) => {
    return Math.ceil(main.getBoundingClientRect().top + main.scrollHeight);
  });
  await page.setViewportSize({ width, height });
}

async function capture(page: Page, heading: string, name: string): Promise<void> {
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(heading);
  const size = page.viewportSize();
  await showWholeStep(page);
  await expectScreenshot(page, name);
  if (size) await page.setViewportSize(size);
}

test.describe("setup wizard", { tag: "@visual" }, () => {
  test("every step", async ({ page, mock }) => {
    await mock.post("/api/mock/reset", { data: { setup: true } });
    await page.goto("/");
    await expectSetupOpen(page);

    await capture(page, "Welcome to Residuum", "setup-welcome");
    await page.getByRole("button", { name: "Next" }).click();

    await page.getByRole("switch", { name: "OpenAI" }).click();
    await capture(page, "Add model providers", "setup-providers");
    await page.getByRole("button", { name: "Next" }).click();

    await expect(page.getByRole("group", { name: "Large" }).getByLabel("Model")).toHaveValue(
      "claude-sonnet-4-6",
    );
    await capture(page, "Assign models", "setup-models");
    await page.getByRole("button", { name: "Next" }).click();

    await page.getByRole("button", { name: "Add fetch" }).click();
    await page.getByRole("button", { name: "Add tavily" }).click();
    await capture(page, "Add tool servers", "setup-tool-servers");
    await page.getByRole("button", { name: "Cancel" }).click();
    await page.getByRole("button", { name: "Next" }).click();

    await capture(page, "Connect chat apps", "setup-connections");
    await page.getByRole("button", { name: "Next" }).click();

    await capture(page, "Save and start", "setup-save");
  });
});
