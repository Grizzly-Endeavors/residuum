import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The All agents scope's Saved keys, Agent-to-agent listener, Notifications
 * and History sections. None holds a legacy panel, so the whole modal is
 * compared.
 */

test.describe(
  "settings: saved keys, listener, notifications and history",
  { tag: "@visual" },
  () => {
    test("Saved keys, with the add key form open", async ({ page }) => {
      await page.goto("/home?settings=_all/keys");
      await expect(page.getByRole("list", { name: "Agent keys" })).toContainText("github_token");
      await expect(page.getByRole("list", { name: "Stored secrets" })).toContainText("openai_key");
      await page.getByRole("button", { name: "Add a key" }).click();
      await page.getByLabel("Name", { exact: true }).fill("stripe_live");
      await page.getByLabel("Value", { exact: true }).fill("1234");
      await page.getByLabel("Description").focus();
      await page.getByLabel("Description").blur();
      await expect(page.getByText(/under 8 characters can't be hidden/)).toBeVisible();
      await expectScreenshot(page, "settings-keys");
    });

    test("Agent-to-agent listener, with a new caller key's token shown", async ({ page }) => {
      await page.goto("/home?settings=_all/listener");
      await expect(page.getByRole("list", { name: "Caller keys" })).toContainText("laptop");
      await page.getByRole("button", { name: "Add a caller key" }).click();
      await page.getByLabel("Name", { exact: true }).fill("phone");
      await page.getByLabel("Description").fill("My phone");
      await page.getByRole("button", { name: "Create key" }).click();
      await expect(page.getByText("Key for phone created.")).toBeVisible();
      await expect(page.getByRole("button", { name: "Copy key" })).toBeFocused();
      await page.getByRole("button", { name: "Copy key" }).blur();
      // The token is minted by the mock from a counter, which other requests also advance.
      await expectScreenshot(page, "settings-listener", {
        mask: [page.locator("code").filter({ hasText: /^rsdm_a2a_mock/ })],
      });
    });

    test("Agent-to-agent listener, switched off", async ({ page }) => {
      await page.goto("/home?settings=_all/listener");
      await page.getByRole("switch", { name: "Let other agents reach this install" }).click();
      await expect(page.getByLabel("Listener port")).toBeDisabled();
      await page.getByRole("switch", { name: "Let other agents reach this install" }).blur();
      await expectScreenshot(page, "settings-listener-off");
    });

    test("Notifications", async ({ page }) => {
      await page.goto("/home?settings=_all/notifications");
      await expect(page.getByText(/aren't available in this version/)).toBeVisible();
      await expectScreenshot(page, "settings-notifications");
    });

    test("History of the install-wide config, with a checkpoint open", async ({ page }) => {
      await page.goto("/home?settings=_all/history");
      await page.getByRole("radio", { name: "Install-wide config" }).click();
      await page.getByRole("button", { name: /^config patch/ }).click();
      await expect(page.getByRole("list", { name: "Files this checkpoint changed" })).toBeVisible();
      await page.getByRole("button", { name: /^config patch/ }).blur();
      await page.mouse.move(0, 0);
      await expectScreenshot(page, "settings-history-install");
    });
  },
);
