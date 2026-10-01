import type { Locator, Page } from "@playwright/test";
import type { PushDevice } from "../../src/lib/generated/PushDevice";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { deliverPush, fakePushService, shownNotifications } from "../support/push";

/**
 * Web Push on this device (design §11): turning it on from Settings →
 * Notifications, what the device is told about, a test send, presence on the
 * hub socket while the window is in front, and the worker's notifications and
 * clicks. The worker exists only in the production build, so these run there
 * (`@preview`), against a stand-in push service (`support/push.ts`).
 */

// Chromium's headless shell, the default, can't show notifications: it reports
// them denied and crashes on a push. Full Chromium, headless, can.
test.use({ channel: "chromium" });

const OVERLAY = "[data-overlay-host]";

function thisDevice(page: Page): Locator {
  return page.getByRole("region", { name: "This device" });
}

async function devices(page: Page): Promise<PushDevice[]> {
  const answer = (await (await page.request.get("/api/hub/push/devices")).json()) as {
    devices: PushDevice[];
  };
  return answer.devices;
}

async function presentDevices(page: Page): Promise<string[]> {
  const answer = (await (await page.request.get("/api/mock/push/presence")).json()) as {
    devices: string[];
  };
  return answer.devices;
}

/** Open Notifications once the worker controls the page, and turn push on as `name`. */
async function turnOn(page: Page, name = "Test phone"): Promise<string> {
  await page.goto("/home?settings=_all/notifications");
  await page.evaluate(() => navigator.serviceWorker.ready);
  const group = thisDevice(page);
  await group.getByLabel("Name for this device").fill(name);
  await group.getByRole("button", { name: "Turn on notifications" }).click();
  await expect(group.getByText("On", { exact: true })).toBeVisible();
  const [device] = await devices(page);
  expect(device?.label).toBe(name);
  return device?.id ?? "";
}

test.describe("notifications on this device", { tag: "@preview" }, () => {
  test.beforeEach(async ({ context }) => {
    await fakePushService(context);
  });

  test("turns on, keeps what it is told about across a reload, sends a test and turns off", async ({
    page,
  }) => {
    await page.goto("/home?settings=_all/notifications");
    await expect(thisDevice(page).getByText("Off", { exact: true })).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await turnOn(page);
    const told = page.getByRole("region", { name: "What to notify about" });
    await expect(told.getByRole("switch", { name: "New inbox items" })).toBeChecked();
    await expect(told.getByRole("switch", { name: "Replies while you're away" })).not.toBeChecked();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await told.getByRole("switch", { name: "Replies while you're away" }).click();
    await told.getByRole("switch", { name: "New inbox items" }).click();
    await expect(told.getByRole("switch", { name: "New inbox items" })).not.toBeChecked();
    await expect
      .poll(async () => (await devices(page))[0]?.preferences)
      .toEqual({
        inbox_item: false,
        agent_failed: true,
        outbound_unreachable: false,
        reply_while_away: true,
      });

    await page.reload();
    await expect(told.getByRole("switch", { name: "Replies while you're away" })).toBeChecked();
    await expect(told.getByRole("switch", { name: "New inbox items" })).not.toBeChecked();
    await expect(thisDevice(page).getByLabel("Device name")).toHaveValue("Test phone");

    await thisDevice(page).getByRole("button", { name: "Send a test notification" }).click();
    await expect(page.getByText("Sent a test notification.", { exact: false })).toBeVisible();
    // The mock's clock stands still while the page's runs, so only the outcome is compared.
    await expect(thisDevice(page).getByText(/^Last notification sent/)).toBeVisible();

    await thisDevice(page).getByRole("button", { name: "Turn off on this device" }).click();
    await expect(thisDevice(page).getByText("Off", { exact: true })).toBeVisible();
    await expect(told).toHaveCount(0);
    expect(await devices(page)).toEqual([]);
  });

  test("reports presence while the window is focused, and clears it on blur", async ({ page }) => {
    const id = await turnOn(page);
    await expect.poll(() => presentDevices(page)).toEqual([id]);

    await page.evaluate(() => window.dispatchEvent(new FocusEvent("blur")));
    await expect.poll(() => presentDevices(page)).toEqual([]);

    await page.evaluate(() => window.dispatchEvent(new FocusEvent("focus")));
    await expect.poll(() => presentDevices(page)).toEqual([id]);

    // Turned off, the device is no longer present.
    await thisDevice(page).getByRole("button", { name: "Turn off on this device" }).click();
    await expect.poll(() => presentDevices(page)).toEqual([]);
  });
});

test.describe("the worker's notifications", { tag: "@preview" }, () => {
  test.beforeEach(async ({ context }) => {
    await fakePushService(context);
  });

  test("every push shows a notification, even one it can't read", async ({ page }) => {
    await page.goto("/home");
    await page.evaluate(() => navigator.serviceWorker.ready);

    await deliverPush(page, {
      v: 1,
      event: "inbox_item",
      agent: "atlas",
      title: "Deploy tomorrow",
      body: "From atlas: Reminder to trigger the deployment pipeline",
      target: "/inbox?item=atlas:mock_1",
      tag: "inbox:atlas:mock_1",
      badge: 2,
    });
    await expect
      .poll(() => shownNotifications(page))
      .toEqual([
        {
          title: "Deploy tomorrow",
          body: "From atlas: Reminder to trigger the deployment pipeline",
          tag: "inbox:atlas:mock_1",
          target: "/inbox?item=atlas:mock_1",
        },
      ]);

    await deliverPush(page, "not the hub's payload");
    await expect
      .poll(async () => (await shownNotifications(page)).map((shown) => shown.title))
      .toContain("Residuum");
  });

  test("a click brings the open window to the notification's target", async ({ page, context }) => {
    await page.goto("/home");
    await page.evaluate(() => navigator.serviceWorker.ready);
    await deliverPush(page, {
      v: 1,
      event: "agent_failed",
      agent: "brittle",
      title: "brittle couldn't start",
      body: "Its settings need fixing.",
      target: "/agent/brittle",
      tag: "failed:brittle",
      badge: 2,
    });
    await expect.poll(async () => (await shownNotifications(page)).length).toBe(1);

    // No browser API clicks a notification. A synthetic click runs the
    // worker's own handler, which can't take focus or hold the event open the
    // way a person's click can, and routes the open window the same way.
    const [worker] = context.serviceWorkers();
    await worker?.evaluate(async () => {
      // The worker's globals, which the page's types don't have.
      const scope = self as unknown as {
        registration: ServiceWorkerRegistration;
        NotificationEvent: new (type: string, init: { notification: Notification }) => Event;
        dispatchEvent: (event: Event) => boolean;
      };
      const [notification] = await scope.registration.getNotifications();
      if (notification === undefined) throw new Error("no notification is showing");
      scope.dispatchEvent(new scope.NotificationEvent("notificationclick", { notification }));
    });

    await expect.poll(() => new URL(page.url()).pathname).toBe("/agent/brittle");
    await expect(page.getByRole("region", { name: "brittle couldn't start" })).toBeVisible();
  });

  const targets: readonly [string, (page: Page) => Locator][] = [
    ["/inbox?item=atlas:mock_1", (p) => p.getByText("Reminder to trigger the deployment pipeline")],
    ["/agent/brittle", (p) => p.getByRole("region", { name: "brittle couldn't start" })],
    ["/agent/atlas/activity", (p) => p.getByRole("heading", { name: "Running now" })],
    ["/home", (p) => p.getByRole("heading", { name: "Home", level: 1 })],
  ];
  for (const [target, landed] of targets) {
    test(`with no window open, a click's new window starts the app at ${target}`, async ({
      page,
      context,
    }) => {
      await page.goto("/home");
      await page.evaluate(() => navigator.serviceWorker.ready);
      await page.close();
      // What the worker's `openWindow(target)` opens: a new window at the target, through the worker.
      const opened = await context.newPage();
      await opened.goto(target);
      await expect(landed(opened)).toBeVisible();
      expect(`${new URL(opened.url()).pathname}${new URL(opened.url()).search}`).toBe(target);
    });
  }
});
