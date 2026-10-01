import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** Activity's baselines: what's running and what finished, and a task its agent can't reach. */

test.describe("activity", { tag: "@visual" }, () => {
  test("what's running and what finished", async ({ page }) => {
    await page.goto("/agent/atlas/activity");
    await expect(page.getByRole("heading", { name: /^Finished/ })).toHaveText("Finished 25+");
    await expectScreenshot(page, "activity");
  });

  test("a task that couldn't be stopped", async ({ page }) => {
    await page.goto("/agent/atlas/activity");
    const running = page.getByRole("region", { name: /^Running now/ });
    await running.getByRole("button", { name: "Stop the task sent to laptop" }).click();
    await expect(running.getByText(/^Couldn't reach laptop to cancel the task\./)).toBeVisible();
    await running.getByText(/^Can't reach laptop for /).scrollIntoViewIfNeeded();
    await expectScreenshot(page, "activity-unreachable-task");
  });
});
