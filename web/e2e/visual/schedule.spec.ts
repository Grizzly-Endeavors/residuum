import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** The Schedule's baselines: an agent's pulses and actions, and a stopped agent's. */

test.describe("schedule", { tag: "@visual" }, () => {
  test("an agent's pulses and scheduled actions", async ({ page }) => {
    await page.goto("/agent/atlas/schedule");
    await expect(page.getByRole("switch", { name: "Inbox check" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Cancel Review open prs" })).toBeVisible();
    await expectScreenshot(page, "schedule");
  });

  test("a stopped agent", async ({ page }) => {
    await page.goto("/agent/drifter/schedule");
    await expect(page.getByRole("button", { name: "Start drifter" })).toBeVisible();
    await expectScreenshot(page, "schedule-stopped");
  });
});
