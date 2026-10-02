import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectSettingsOpen } from "../support/lazy";

/** The All agents scope's General, Residuum Cloud, Updates, Session limits and Diagnostics sections. */

const HUB_CONFIG = "/api/hub/config/raw";
const OVERLAY = "[data-overlay-host]";

const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const save = (page: Page): Promise<void> =>
  saveBar(page).getByRole("button", { name: "Save changes" }).click();

async function hubConfig(page: Page): Promise<string> {
  return (await page.request.get(HUB_CONFIG)).text();
}

async function openSection(page: Page, section: string): Promise<void> {
  await page.goto(`/home?settings=_all/${section}`);
  await expectSettingsOpen(page);
}

/** Record what the page opens in a new tab, since the relay's sign-in page can't load here. */
async function recordOpenedTabs(page: Page): Promise<() => Promise<string[]>> {
  await page.evaluate(() => {
    const opened: string[] = [];
    Object.assign(window, { openedTabs: opened });
    window.open = (url) => {
      opened.push(String(url));
      return null;
    };
  });
  return () => page.evaluate(() => (window as unknown as { openedTabs: string[] }).openedTabs);
}

test.describe("General", () => {
  test("a timezone and the gateway port are saved with Save changes and kept across a reload", async ({
    page,
  }) => {
    await openSection(page, "general");
    await expect(page.getByLabel("Timezone")).toHaveValue("America/New_York");
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByLabel("Timezone").selectOption("Europe/Berlin");
    await page.getByRole("button", { name: "More options" }).click();
    await page.getByLabel("Port", { exact: true }).fill("7711");
    await expect(saveBar(page)).toContainText("You have unsaved changes.");
    await expectNoAxeViolations(page, { within: OVERLAY });
    expect(await hubConfig(page)).toContain('timezone = "America/New_York"');

    await save(page);
    await expect(
      page.getByRole("status").filter({ hasText: "Saved the install-wide config.toml." }),
    ).toBeVisible();
    await expect(saveBar(page)).toBeHidden();
    const written = await hubConfig(page);
    expect(written).toContain('timezone = "Europe/Berlin"');
    expect(written).toContain("port = 7711");

    await page.reload();
    await expect(page.getByLabel("Timezone")).toHaveValue("Europe/Berlin");
  });

  test("Discard brings the saved timezone back", async ({ page }) => {
    await openSection(page, "general");

    await page.getByLabel("Timezone").selectOption("Europe/Berlin");
    await expect(saveBar(page)).toBeVisible();

    await saveBar(page).getByRole("button", { name: "Discard" }).click();
    await expect(page.getByLabel("Timezone")).toHaveValue("America/New_York");
    await expect(saveBar(page)).toBeHidden();
  });
});

test.describe("Residuum Cloud", () => {
  test("with no account it signs in at the relay the config names, and the account shows up when the relay hands the token back", async ({
    page,
    mock,
  }) => {
    await openSection(page, "cloud");
    await expect(page.getByText("Not connected")).toBeVisible();
    await expect(page.getByText(/Opens agent-residuum.com in a new tab/)).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });
    const opened = await recordOpenedTabs(page);

    await page.getByRole("button", { name: "Connect to Residuum Cloud" }).click();
    expect(await opened()).toEqual(["https://agent-residuum.com/connect?port=7700"]);

    await page.getByRole("button", { name: "More options" }).click();
    await page.getByLabel("Relay URL").fill("ws://127.0.0.1:8080/tunnel/register");
    await page.getByRole("button", { name: "Connect to Residuum Cloud" }).click();
    expect(await opened()).toEqual([
      "https://agent-residuum.com/connect?port=7700",
      "http://127.0.0.1:8080/connect?port=7700",
    ]);
    await saveBar(page).getByRole("button", { name: "Discard" }).click();

    await mock.post("/api/mock/cloud-callback");
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(page.getByText("mock-user")).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });
  });

  test("a pasted token connects, Disconnect and Reconnect act at once with nothing staged, and Remove account is staged", async ({
    page,
  }) => {
    await openSection(page, "cloud");
    await page.getByRole("button", { name: "Use a token instead" }).click();
    await page.getByLabel("Tunnel token", { exact: true }).fill("rst_abc123");
    await page.getByRole("button", { name: "Connect with token" }).click();
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    expect(await hubConfig(page)).toContain('token = "secret:cloud_token"');

    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
    await expect(saveBar(page)).toBeHidden();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("button", { name: "Reconnect" }).click();
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(saveBar(page)).toBeHidden();

    await page.getByRole("button", { name: "Disconnect" }).click();
    await page.getByRole("button", { name: "Remove account" }).click();
    await expect(page.getByText("The account is removed when you save changes.")).toBeVisible();
    await expect(saveBar(page)).toContainText("You have unsaved changes.");
    await save(page);
    await expect(page.getByText("Not connected")).toBeVisible();
    expect(await hubConfig(page)).not.toContain("secret:cloud_token");
  });

  test("a connection that is still starting offers Cancel", async ({ page, mock }) => {
    await mock.post("/api/mock/cloud-callback");
    await mock.post("/api/mock/cloud", { data: { tunnel: "connecting" } });
    await openSection(page, "cloud");
    await expect(page.getByText("Connecting…")).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByText("Disconnected", { exact: true })).toBeVisible();
  });

  test("seen through Residuum Cloud there is no Disconnect, and the page says why", async ({
    page,
    mock,
  }) => {
    await mock.post("/api/mock/cloud-callback");
    await mock.post("/api/mock/cloud", { data: { via_tunnel: true } });
    await openSection(page, "cloud");

    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expect(page.getByText(/can't be disconnected from here/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Disconnect" })).toBeHidden();
    await expectNoAxeViolations(page, { within: OVERLAY });
  });
});

test.describe("Updates", () => {
  test("Check for updates reads the latest version and says Residuum is up to date", async ({
    page,
  }) => {
    await openSection(page, "updates");
    await expect(page.getByText("Not checked yet")).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("button", { name: "Check for updates" }).click();

    await expect(page.getByText("Up to date")).toBeVisible();
    await expect(page.getByText("Checked", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Update and restart" })).toBeHidden();
    await expectNoAxeViolations(page, { within: OVERLAY });
  });
});

test.describe("Session limits and Diagnostics", () => {
  test("a diagnostics switch is staged, saved to the hub's config, and still on after a reload", async ({
    page,
  }) => {
    await openSection(page, "diagnostics");
    const report = page.getByRole("switch", { name: "Report errors automatically" });
    await expect(report).not.toBeChecked();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await report.click();
    await page.getByLabel("Log detail").selectOption("trace");
    await expect(saveBar(page)).toContainText("You have unsaved changes.");
    await save(page);
    await expect(saveBar(page)).toBeHidden();
    const written = await hubConfig(page);
    expect(written).toContain("auto_error_reporting = true");
    expect(written).toContain('log_level = "trace"');

    await page.reload();
    await expect(page.getByRole("switch", { name: "Report errors automatically" })).toBeChecked();
    await expect(page.getByLabel("Log detail")).toHaveValue("trace");
  });

  test("the session limits show the numbers, flag a value that blocks work, and save", async ({
    page,
  }) => {
    await openSection(page, "limits");
    await expect(page.getByLabel("Turns at once")).toHaveAttribute("placeholder", "3");
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByLabel("Turns at once").fill("0");
    await expect(page.getByText(/can never run a turn/)).toBeVisible();
    await page.getByLabel("Turns at once").fill("5");
    await expect(page.getByText(/can never run a turn/)).toBeHidden();
    await save(page);

    await expect(saveBar(page)).toBeHidden();
    expect(await hubConfig(page)).toContain("max_concurrent = 5");
  });
});
