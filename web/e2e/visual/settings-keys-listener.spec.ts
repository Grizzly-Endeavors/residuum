import type { Page } from "@playwright/test";
import { MOCK_VAPID_PUBLIC_KEY } from "../../mock/push";
import { expect, test } from "../support/fixtures";
import { fakePushService } from "../support/push";
import { expectScreenshot } from "../support/screenshot";

/**
 * The All agents scope's Saved keys, Agent-to-agent listener, Notifications
 * and History sections. None holds a legacy panel, so the whole modal is
 * compared.
 */

/**
 * Open `url` once the hub socket is up (the overview is fetched when it says
 * hello), so a slow connection's banner isn't shot.
 */
async function openConnected(page: Page, url: string): Promise<void> {
  const hello = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/hub/overview");
  await page.goto(url);
  await hello;
  await expect(page.getByText("Can't reach Residuum.")).toBeHidden();
}

test.describe(
  "settings: saved keys, listener, notifications and history",
  { tag: "@visual" },
  () => {
    test("Saved keys, with the add key form open", async ({ page }) => {
      await openConnected(page, "/home?settings=_all/keys");
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
      await openConnected(page, "/home?settings=_all/listener");
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
      await openConnected(page, "/home?settings=_all/listener");
      await page.getByRole("switch", { name: "Let other agents reach this install" }).click();
      await expect(page.getByLabel("Listener port")).toBeDisabled();
      await page.getByRole("switch", { name: "Let other agents reach this install" }).blur();
      await expectScreenshot(page, "settings-listener-off");
    });

    test("Notifications, turned on here, with another device", async ({ page, context }) => {
      await fakePushService(context, { registration: true });
      await page.request.put("/api/hub/push/devices", {
        data: {
          subscription: {
            endpoint: "https://push.example.test/send/tablet",
            keys: { p256dh: MOCK_VAPID_PUBLIC_KEY, auth: "AAAAAAAAAAAAAAAAAAAAAA" },
          },
          label: "Old tablet",
        },
      });
      await openConnected(page, "/home?settings=_all/notifications");
      const thisDevice = page.getByRole("region", { name: "This device" });
      await thisDevice.getByLabel("Name for this device").fill("Pixel 7");
      await thisDevice.getByRole("button", { name: "Turn on notifications" }).click();
      await expect(thisDevice.getByLabel("Device name")).toHaveValue("Pixel 7");
      await expect(page.getByRole("list", { name: "Other devices" })).toContainText("Old tablet");
      await expectScreenshot(page, "settings-notifications");
    });

    test("Notifications, before they are turned on", async ({ page, context }) => {
      await fakePushService(context, { registration: true });
      await openConnected(page, "/home?settings=_all/notifications");
      await expect(page.getByRole("button", { name: "Turn on notifications" })).toBeVisible();
      await expect(page.getByText("No other devices get notifications.")).toBeVisible();
      await expectScreenshot(page, "settings-notifications-off");
    });

    test("History of the install-wide config, with a checkpoint open", async ({ page }) => {
      await openConnected(page, "/home?settings=_all/history");
      await page.getByRole("radio", { name: "Install-wide config" }).click();
      await page.getByRole("button", { name: /^config patch/ }).click();
      await expect(page.getByRole("list", { name: "Files this checkpoint changed" })).toBeVisible();
      await page.getByRole("button", { name: /^config patch/ }).blur();
      await page.mouse.move(0, 0);
      await expectScreenshot(page, "settings-history-install");
    });
  },
);
