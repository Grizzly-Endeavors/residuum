import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** An agent's Advanced → Tool servers and Advanced → Agent-to-agent sections. */
test.describe("settings: tool servers and agent-to-agent", { tag: "@visual" }, () => {
  test("Tool servers, with a server open for editing", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/servers");
    await expect(page.getByRole("list", { name: "Catalog" })).toBeVisible();
    await page.getByRole("button", { name: "Edit remote-search" }).click();
    await expect(page.getByLabel("Headers")).toBeVisible();
    await page.getByRole("button", { name: "Edit remote-search" }).blur();
    await expectScreenshot(page, "settings-servers");
  });

  test("Agent-to-agent for a running agent", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/a2a");
    await expect(page.getByText("http://127.0.0.1:7702/agents/atlas")).toBeVisible();
    await expect(page.getByRole("list", { name: "Remote agents" })).toBeVisible();
    await expectScreenshot(page, "settings-a2a");
  });

  test("Agent-to-agent for a stopped agent", async ({ page }) => {
    await page.goto("/agent/drifter?settings=drifter/a2a");
    await expect(page.getByText("No remote agents yet. List one in a2a.json.")).toBeVisible();
    await expectScreenshot(page, "settings-a2a-stopped");
  });
});
